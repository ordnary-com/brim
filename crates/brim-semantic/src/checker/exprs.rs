//! Expressions, including generic calls, struct literals and enum variants.
//!
//! Type arguments are inferred: first from the type the context expects
//! (`let x: Option[i64] = None`), then from the arguments, left to right.
//! Each argument is checked with the type its parameter has once the type
//! arguments known so far are filled in, so `wrap(None)` works when `wrap`
//! takes an `Option[i64]`.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};
use brim_stdlib::Prim;

use super::{Checker, FnCtx, error_expr, plural, supplied};
use crate::checked::{
    BinaryOp, CheckedExpr, CheckedExprKind, CheckedFn, CheckedPlace, FnId, Local, LocalId, UnaryOp,
};
use crate::suggest;
use crate::{Def, EnumId, Type};

/// The `self` of a method call.
enum MethodReceiver {
    Value(CheckedExpr),
    /// For `mut self` methods.
    Place(CheckedPlace),
}

impl MethodReceiver {
    fn ty(&self) -> Type {
        match self {
            MethodReceiver::Value(v) => v.ty,
            MethodReceiver::Place(p) => p.ty,
        }
    }
}

/// What a generic item is, for "cannot infer" messages.
struct Generic<'n> {
    /// "function `wrap`", "struct `Box`", "`None`", ...
    what: String,
    params: &'n [String],
}

impl<'a> Checker<'a> {
    pub(super) fn check_expr(&mut self, expr: &ast::Expr, ctx: &mut FnCtx) -> CheckedExpr {
        self.check_expr_with(expr, None, ctx)
    }

    /// Checks `expr`. `expected` is the type the context needs, if known; it
    /// is only used to infer type arguments, and mismatches are reported by
    /// the caller.
    pub(super) fn check_expr_with(
        &mut self,
        expr: &ast::Expr,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let span = expr.span;
        let (kind, ty) = match &expr.kind {
            ast::ExprKind::Float(bits) => (CheckedExprKind::Float(*bits), Type::F64),
            ast::ExprKind::Int(value) => {
                // A literal takes the integer type the context expects.
                let ty = expected.filter(|t| t.is_int()).unwrap_or(Type::I64);
                match int_literal(*value, false, ty) {
                    Some(v) => (CheckedExprKind::Int(v), ty),
                    None => {
                        self.int_out_of_range(*value, false, ty, span);
                        return error_expr(span);
                    }
                }
            }
            ast::ExprKind::Bool(value) => (CheckedExprKind::Bool(*value), Type::Bool),
            ast::ExprKind::Str(value) => (CheckedExprKind::Str(value.clone()), Type::String),
            ast::ExprKind::Name(ident) => return self.check_name(ident, expected, ctx),
            ast::ExprKind::Branch(stmt) => return self.check_branch_value(stmt, expected, ctx).0,
            ast::ExprKind::FString(parts) => return self.check_fstring(parts, span, ctx),
            ast::ExprKind::Tuple(items) => return self.check_tuple(items, span, expected, ctx),
            ast::ExprKind::Lambda { params, ret, body } => {
                return self.check_lambda(params, ret.as_ref(), body, span, ctx);
            }
            ast::ExprKind::CallValue { callee, args } => {
                let callee = self.check_expr(callee, ctx);
                if callee.ty != Type::Error && self.fn_type_parts(callee.ty).is_none() {
                    for arg in args {
                        self.check_expr(arg, ctx);
                    }
                    let shown = self.show(callee.ty);
                    self.diags.push(
                        Diagnostic::error("only functions can be called")
                            .with_code("E0107")
                            .with_primary(callee.span, format!("this is a `{shown}`")),
                    );
                    return error_expr(span);
                }
                let shown = self.show(callee.ty);
                return self.call_value(callee, &shown, args, span, ctx);
            }
            ast::ExprKind::Unary { op, operand, .. } => {
                return self.check_unary(*op, operand, span, expected, ctx);
            }
            ast::ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => return self.check_binary(*op, *op_span, lhs, rhs, span, expected, ctx),
            ast::ExprKind::Call { callee, args } => {
                return self.check_call(callee, args, span, expected, ctx);
            }
            ast::ExprKind::StructLit { name, fields } => {
                return self.check_struct_lit(name, fields, span, expected, ctx);
            }
            ast::ExprKind::Field { base, field } => {
                if let Some(e) = self.enum_name(base, ctx) {
                    return self.check_variant_name(e, field, None, span, expected, ctx);
                }
                if let Some(target) = self.module_alias(base, ctx) {
                    if let Some(value) = self.module_const(target, field, span) {
                        return value;
                    }
                    let module = self.module_names[target].clone();
                    let scope = &self.scopes[target];
                    let what = if scope.own_fns.contains_key(field.name.as_str()) {
                        "a function"
                    } else if scope.own_types.contains_key(&field.name) {
                        "a type"
                    } else {
                        ""
                    };
                    let diag = if what.is_empty() {
                        Diagnostic::error(format!("module `{module}` has no `{}`", field.name))
                            .with_code("E0200")
                            .with_primary(field.span, "not found")
                    } else {
                        Diagnostic::error(format!("`{}` is {what}, not a value", field.name))
                            .with_code("E0214")
                            .with_primary(span, "")
                    };
                    self.diags.push(diag);
                    return error_expr(span);
                }
                let base = self.check_expr(base, ctx);
                let Some((index, ty)) = self.field(base.ty, field, base.span) else {
                    return error_expr(span);
                };
                (
                    CheckedExprKind::Field {
                        base: Box::new(base),
                        index,
                    },
                    ty,
                )
            }
            ast::ExprKind::Try(operand) => return self.check_try(operand, span, ctx),
            ast::ExprKind::ListLit(items) => {
                return self.check_list_lit(items, span, expected, ctx);
            }
            ast::ExprKind::MapLit(entries) => {
                return self.check_map_lit(entries, span, expected, ctx);
            }
            ast::ExprKind::Index { base, index } => {
                return self.check_index(base, index, span, ctx);
            }
            ast::ExprKind::MethodCall { base, name, args } => {
                if let Some(target) = self.module_alias(base, ctx) {
                    return self.check_module_call(target, name, args, span, expected, ctx);
                }
                if let Some(def) = self.type_name_of(base, ctx) {
                    return self.check_static_call(def, name, args, span, expected, ctx);
                }
                if let Some(result) = self.method_on_list(base, name, args, span, ctx) {
                    return result;
                }
                if let Some(result) = self.method_on_box(base, name, args, span, ctx) {
                    return result;
                }
                if let Some(result) = self.method_on_param(base, name, args, span, ctx) {
                    return result;
                }
                if let Some(result) = self.method_on_value(base, name, args, span, expected, ctx) {
                    return result;
                }
                let base = self.check_expr(base, ctx);
                for arg in args {
                    self.check_expr(arg, ctx);
                }
                if base.ty != Type::Error {
                    let ty = self.show(base.ty);
                    self.diags.push(
                        Diagnostic::error(format!("`{ty}` has no method `{}`", name.name))
                            .with_code("E0231")
                            .with_primary(name.span, "unknown method")
                            .with_secondary(base.span, format!("this is `{ty}`"))
                            .with_note("methods are not supported yet; use a function instead"),
                    );
                }
                return error_expr(span);
            }
        };
        CheckedExpr { kind, ty, span }
    }

    /// `operand?`. The built-in enums are `Option` (`EnumId(0)`) and
    /// `Result` (`EnumId(1)`); in both, variant 0 holds the value.
    fn check_try(&mut self, operand: &ast::Expr, span: Span, ctx: &mut FnCtx) -> CheckedExpr {
        let operand = self.check_expr(operand, ctx);
        if operand.ty == Type::Error {
            return error_expr(span);
        }
        let question = Span::new(span.end.saturating_sub(1), span.end);
        let Some((e, args)) = self.as_enum(operand.ty).filter(|(e, _)| e.0 <= 1) else {
            let ty = self.show(operand.ty);
            self.diags.push(
                Diagnostic::error(format!("`?` cannot be applied to a value of type `{ty}`"))
                    .with_code("E0232")
                    .with_primary(question, "")
                    .with_secondary(operand.span, format!("this is `{ty}`"))
                    .with_note("`?` works on `Option` and `Result` values"),
            );
            return error_expr(span);
        };
        let is_result = e.0 == 1;
        let mut convert = None;
        let mut conversion_help = None;
        let compatible = match self.as_enum(ctx.ret) {
            _ if ctx.ret == Type::Error => true,
            Some((ret_enum, ret_args)) if ret_enum == e => {
                if !is_result || ret_args[1] == args[1] || ret_args[1] == Type::Error {
                    true
                } else {
                    match self.error_conversion(args[1], ret_args[1], question) {
                        Ok(found) => {
                            convert = Some((ret_args[1], found));
                            true
                        }
                        Err(help) => {
                            conversion_help = help;
                            false
                        }
                    }
                }
            }
            _ => false,
        };
        if !compatible && conversion_help.as_deref() == Some("") {
            // Already reported.
            return error_expr(span);
        }
        if !compatible {
            let (kind, needed) = if is_result {
                ("an `Err`", format!("`Result[..., {}]`", self.show(args[1])))
            } else {
                ("`None`", "`Option[...]`".to_string())
            };
            let ret = self.show(ctx.ret);
            let mut diag = Diagnostic::error(format!(
                "`?` cannot return {kind} from a function that returns `{ret}`"
            ))
            .with_code("E0233")
            .with_primary(question, format!("this returns {kind} early"));
            if let Some(ret_span) = ctx.ret_span {
                diag = diag.with_secondary(ret_span, "the function's return type");
            }
            diag = diag.with_help(match conversion_help {
                Some(help) => help,
                None => {
                    format!("the function must return {needed}, or handle the value with `match`")
                }
            });
            self.diags.push(diag);
            return error_expr(span);
        }
        CheckedExpr {
            ty: args[0],
            kind: CheckedExprKind::Try {
                operand: Box::new(operand),
                convert,
            },
            span,
        }
    }

    /// How `?` turns an error of type `from` into the function's error type
    /// `to`: `to` must be an enum with exactly one variant whose only value
    /// is a `from`. Returns that variant, or else the help to show (`None`
    /// for the default one, empty when an error was already reported).
    fn error_conversion(
        &mut self,
        from: Type,
        to: Type,
        question: Span,
    ) -> Result<usize, Option<String>> {
        let from_shown = self.show(from);
        let to_shown = self.show(to);
        let Some((e, args)) = self.as_enum(to).filter(|(e, _)| e.0 > 1) else {
            // Only enums can wrap other errors: the default help applies.
            return Err(None);
        };
        let mut candidates: Vec<(usize, String)> = Vec::new();
        let variants: Vec<(String, Vec<Type>)> = self.enums[e.0]
            .variants
            .iter()
            .map(|v| (v.name.clone(), v.fields.clone()))
            .collect();
        for (i, (name, fields)) in variants.into_iter().enumerate() {
            if let [field] = fields[..]
                && self.types.subst(field, &args) == from
            {
                candidates.push((i, name));
            }
        }
        match candidates.as_slice() {
            [] => Err(Some(format!(
                "add a variant like `Wrapped({from_shown})` to `{to_shown}`, and `?` wraps the error in it"
            ))),
            [(index, _)] => Ok(*index),
            _ => {
                let names: Vec<String> = candidates.iter().map(|(_, n)| format!("`{n}`")).collect();
                self.diags.push(
                    Diagnostic::error(format!(
                        "`?` cannot choose how to convert `{from_shown}` into `{to_shown}`"
                    ))
                    .with_code("E0233")
                    .with_primary(question, "")
                    .with_note(format!(
                        "the variants {} each hold a `{from_shown}`",
                        names.join(" and ")
                    ))
                    .with_help("wrap the error yourself with `match`"),
                );
                Err(Some(String::new()))
            }
        }
    }

    /// A method call on a list, if `base` is one. The receiver's type is
    /// found without reporting errors, which the full check does later.
    fn method_on_list(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let errors = self.diags.len();
        let probe = self.check_expr(base, ctx);
        // Discard what the probe reported; the real check reports it again.
        self.diags.truncate(errors);
        if probe.ty == Type::String {
            return Some(self.check_str_method(base, name, args, span, ctx));
        }
        if self.as_map(probe.ty).is_some() {
            return self.check_map_method(base, name, args, span, ctx);
        }
        self.as_list(probe.ty)?;
        self.check_list_method(base, name, args, span, ctx)
    }

    /// If `expr` is a plain name that refers to a declared struct or enum
    /// (and not to a variable), that type.
    fn type_name_of(&mut self, expr: &ast::Expr, ctx: &FnCtx) -> Option<Def> {
        match &expr.kind {
            ast::ExprKind::Name(ident) => {
                if ctx.is_variable(&ident.name) {
                    return None;
                }
                match self.type_by_name.get(&ident.name) {
                    Some(def @ (Def::Struct(_) | Def::Enum(_))) => Some(*def),
                    _ => None,
                }
            }
            // `module.Type`
            ast::ExprKind::Field { base, field } => {
                let target = self.module_alias(base, ctx)?;
                let def = self.scopes[target].own_types.get(&field.name).copied()?;
                if !self.def_public(def) {
                    let module = self.module_names[target].clone();
                    self.private_item(&field.name, &module, field.span);
                }
                Some(def)
            }
            _ => None,
        }
    }

    /// The module `expr` names, if it is an imported module's name (and not
    /// a variable).
    fn module_alias(&self, expr: &ast::Expr, ctx: &FnCtx) -> Option<usize> {
        let ast::ExprKind::Name(ident) = &expr.kind else {
            return None;
        };
        if ctx.is_variable(&ident.name) {
            return None;
        }
        self.aliases.get(&ident.name).copied()
    }

    /// `module.function(args)`.
    fn check_module_call(
        &mut self,
        target: usize,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let module = self.module_names[target].clone();
        let Some(id) = self.scopes[target].own_fns.get(name.name.as_str()).copied() else {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let names: Vec<&str> = self.scopes[target].own_fns.keys().copied().collect();
            let mut diag =
                Diagnostic::error(format!("module `{module}` has no function `{}`", name.name))
                    .with_code("E0201")
                    .with_primary(name.span, "not found");
            if let Some(similar) = suggest::similar(&name.name, names) {
                diag = diag.with_help(format!("did you mean `{similar}`?"));
            }
            self.diags.push(diag);
            return error_expr(span);
        };
        if !self.sigs[id.0].public {
            self.private_item(&name.name, &module, name.span);
        }
        let arg_refs: Vec<&ast::Expr> = args.iter().collect();
        self.finish_call(id, None, &arg_refs, span, expected, ctx)
    }

    /// `Type.name(args)`: an enum variant, or a method without `self`.
    fn check_static_call(
        &mut self,
        def: Def,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        if let Def::Enum(e) = def
            && self.enums[e.0].variants.iter().any(|v| v.name == name.name)
        {
            return self.check_variant_name(e, name, Some(&args.to_vec()), span, expected, ctx);
        }
        let Some(&id) = self.methods.get(&(def, name.name.clone())) else {
            if let Def::Enum(e) = def {
                // Reports the unknown variant.
                return self.check_variant_name(e, name, Some(&args.to_vec()), span, expected, ctx);
            }
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.unknown_method(def, name);
            return error_expr(span);
        };
        self.check_method_visible(id, name);
        if self.sigs[id.0].receiver.is_some() {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let type_name = self.def_name(def).to_string();
            self.diags.push(
                Diagnostic::error(format!("`{}` needs a `{type_name}` value", name.name))
                    .with_code("E0240")
                    .with_primary(span, "called without a value")
                    .with_secondary(self.sigs[id.0].name_span, "the method takes `self`")
                    .with_help(format!(
                        "call it on a value: `{}.{}(...)`",
                        type_name.to_lowercase(),
                        name.name
                    )),
            );
            return error_expr(span);
        }
        let arg_refs: Vec<&ast::Expr> = args.iter().collect();
        self.finish_call(id, None, &arg_refs, span, expected, ctx)
    }

    /// Reports a call of another module's method that is not `pub`.
    fn check_method_visible(&mut self, id: FnId, name: &ast::Ident) {
        let sig = &self.sigs[id.0];
        if sig.module != self.module && !sig.public {
            let module = self.module_names[sig.module].clone();
            self.private_item(&name.name, &module, name.span);
        }
    }

    fn unknown_method(&mut self, def: Def, name: &ast::Ident) {
        let type_name = self.def_name(def).to_string();
        let names: Vec<&str> = self
            .methods
            .keys()
            .filter(|(d, _)| *d == def)
            .map(|(_, n)| n.as_str())
            .collect();
        let mut diag = Diagnostic::error(format!("`{type_name}` has no method `{}`", name.name))
            .with_code("E0231")
            .with_primary(name.span, "unknown method");
        if let Some(span) = self.def_span(def) {
            diag = diag.with_secondary(span, format!("`{type_name}` is declared here"));
        }
        let built_in = self.def_span(def).is_none();
        diag = match suggest::similar(&name.name, names.iter().copied()) {
            Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
            None if built_in => diag.with_note(format!(
                "`{type_name}` is built in and has no methods; use `match`"
            )),
            None => diag.with_help(format!("add it in an `impl {type_name} {{ ... }}` block")),
        };
        self.diags.push(diag);
    }

    /// `value.name(args)` where `value` is a struct or enum with that method.
    /// `b.value()`, if `base` is a `Box`.
    fn method_on_box(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let errors = self.diags.len();
        let probe = self.check_expr(base, ctx);
        self.diags.truncate(errors);
        let Type::Named(type_id) = probe.ty else {
            return None;
        };
        let named = self.types.get(type_id).clone();
        if named.def != Def::Box {
            return None;
        }
        let base = self.check_expr(base, ctx);
        for arg in args {
            self.check_expr(arg, ctx);
        }
        if name.name != "value" {
            let ty = self.show(base.ty);
            self.diags.push(
                Diagnostic::error(format!("`{ty}` has no method `{}`", name.name))
                    .with_code("E0231")
                    .with_primary(name.span, "unknown method")
                    .with_note("a `Box` has one method: `value()`, the value inside it"),
            );
            return Some(error_expr(span));
        }
        if !args.is_empty() {
            self.diags.push(
                Diagnostic::error(format!(
                    "`value` takes 0 arguments but {} supplied",
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(span, "expected 0 arguments"),
            );
            return Some(error_expr(span));
        }
        Some(CheckedExpr {
            ty: named.args[0],
            kind: CheckedExprKind::BoxValue(Box::new(base)),
            span,
        })
    }

    /// `value.method()` where `value`'s type is a type parameter: a method
    /// of one of its bounds.
    fn method_on_param(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let errors = self.diags.len();
        let probe = self.check_expr(base, ctx);
        self.diags.truncate(errors);
        if !matches!(probe.ty, Type::Param(_)) {
            return None;
        }
        let receiver = self.check_expr(base, ctx);
        Some(self.trait_method_call(receiver, name, args, span, ctx))
    }

    /// `Box(value)`.
    fn check_box_new(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let inner_expected = expected.and_then(|ty| match ty {
            Type::Named(id) if self.types.get(id).def == Def::Box => {
                Some(self.types.get(id).args[0])
            }
            _ => None,
        });
        let [arg] = args else {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "`Box` takes 1 argument but {} supplied",
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(callee.span.to(span), "expected 1 argument"),
            );
            return error_expr(span);
        };
        let value = self.check_expr_with(arg, inner_expected, ctx);
        if value.ty == Type::Error {
            return error_expr(span);
        }
        if value.ty == Type::Void {
            self.diags.push(
                Diagnostic::error("`Box` needs a value")
                    .with_code("E0206")
                    .with_primary(value.span, "this has no value"),
            );
            return error_expr(span);
        }
        CheckedExpr {
            ty: self.types.intern(Def::Box, vec![value.ty]),
            kind: CheckedExprKind::BoxNew(Box::new(value)),
            span,
        }
    }

    fn method_on_value(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let errors = self.diags.len();
        let probe = self.check_expr(base, ctx);
        self.diags.truncate(errors);
        let Type::Named(type_id) = probe.ty else {
            return None;
        };
        let def = self.types.get(type_id).def;
        if !matches!(def, Def::Struct(_) | Def::Enum(_)) {
            return None;
        }
        let Some(&id) = self.methods.get(&(def, name.name.clone())) else {
            let receiver = self.check_expr(base, ctx);
            // `button.on_click(3)`: a field holding a function.
            if let Some((_, args_of)) = self.as_struct(receiver.ty)
                && let Some(field) = self.structs[match def {
                    Def::Struct(s) => s.0,
                    _ => unreachable!("a struct"),
                }]
                .fields
                .iter()
                .position(|f| f.name == name.name)
            {
                let Def::Struct(s) = def else {
                    unreachable!("a struct")
                };
                let field_ty = self.structs[s.0].fields[field].ty;
                let field_ty = self.types.subst(field_ty, &args_of);
                if self.fn_type_parts(field_ty).is_some() {
                    let callee = CheckedExpr {
                        span: receiver.span.to(name.span),
                        ty: field_ty,
                        kind: CheckedExprKind::Field {
                            base: Box::new(receiver),
                            index: field,
                        },
                    };
                    return Some(self.call_value(callee, &name.name, args, span, ctx));
                }
            }
            for arg in args {
                self.check_expr(arg, ctx);
            }
            if receiver.ty != Type::Error {
                self.unknown_method(def, name);
            }
            return Some(error_expr(span));
        };
        self.check_method_visible(id, name);
        let arg_refs: Vec<&ast::Expr> = args.iter().collect();
        let receiver = match self.sigs[id.0].receiver {
            None => {
                let type_name = self.def_name(def).to_string();
                self.diags.push(
                    Diagnostic::error(format!("`{}` does not take `self`", name.name))
                        .with_code("E0240")
                        .with_primary(name.span, "")
                        .with_secondary(self.sigs[id.0].name_span, "declared here")
                        .with_help(format!(
                            "call it on the type: `{type_name}.{}(...)`",
                            name.name
                        )),
                );
                for arg in args {
                    self.check_expr(arg, ctx);
                }
                return Some(error_expr(span));
            }
            Some(false) => MethodReceiver::Value(self.check_expr(base, ctx)),
            Some(true) => {
                let action = format!("call `{}` on", name.name);
                match self.check_place(base, ctx, &action) {
                    Some(place) => MethodReceiver::Place(place),
                    None => {
                        for arg in args {
                            self.check_expr(arg, ctx);
                        }
                        return Some(error_expr(span));
                    }
                }
            }
        };
        Some(self.finish_call(id, Some(receiver), &arg_refs, span, expected, ctx))
    }

    /// Checks the arguments of a call of function `id` (with `receiver` as
    /// `self` for a method) and builds the call.
    fn finish_call(
        &mut self,
        id: FnId,
        receiver: Option<MethodReceiver>,
        args: &[&ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let sig = &self.sigs[id.0];
        let params: Vec<(Type, Span)> = sig.params.iter().map(|p| (p.ty, p.span)).collect();
        let (ret, name_span, name) = (sig.ret, sig.name_span, sig.name.clone());
        let is_method = sig.method;
        let type_params = sig.type_params.clone();
        let mut bindings: Vec<Option<Type>> = vec![None; type_params.len()];
        // `self` comes first and determines the `impl`'s type arguments.
        let skip = usize::from(receiver.is_some());
        if let Some(receiver) = &receiver {
            self.types.unify(params[0].0, receiver.ty(), &mut bindings);
        }
        if let Some(expected) = expected {
            self.types.unify(ret, expected, &mut bindings);
        }
        let param_types: Vec<Type> = params[skip..].iter().map(|p| p.0).collect();
        let checked = self.infer_from(&mut bindings, &param_types, args, ctx);
        if checked.len() != params.len() - skip {
            self.diags.push(
                Diagnostic::error(format!(
                    "{} `{name}` takes {} but {} supplied",
                    if is_method { "method" } else { "function" },
                    plural(params.len() - skip, "argument"),
                    supplied(checked.len())
                ))
                .with_code("E0205")
                .with_primary(
                    span,
                    format!("expected {}", plural(params.len() - skip, "argument")),
                )
                .with_secondary(name_span, format!("`{name}` is defined here")),
            );
            return error_expr(span);
        }
        let generic = Generic {
            what: format!("{} `{name}`", if is_method { "method" } else { "function" }),
            params: &type_params,
        };
        let Some(type_args) = self.require_inferred(bindings, generic, &checked, span) else {
            return error_expr(span);
        };
        let bounds = self.sigs[id.0].bounds.clone();
        let short = name.rsplit('.').next().unwrap_or(&name).to_string();
        self.check_bounds(&short, &bounds, &type_args, span);
        for (arg, (ty, param_span)) in checked.iter().zip(&params[skip..]) {
            let ty = self.types.subst(*ty, &type_args);
            self.expect_type(arg, ty, Some(*param_span), "parameter declared here");
        }
        let ret = self.types.subst(ret, &type_args);
        let kind = match receiver {
            None => CheckedExprKind::Call {
                func: id,
                type_args,
                args: checked,
            },
            Some(MethodReceiver::Value(value)) => {
                let mut all = Vec::with_capacity(checked.len() + 1);
                all.push(value);
                all.extend(checked);
                CheckedExprKind::Call {
                    func: id,
                    type_args,
                    args: all,
                }
            }
            Some(MethodReceiver::Place(place)) => CheckedExprKind::CallMut {
                func: id,
                type_args,
                place,
                args: checked,
            },
        };
        CheckedExpr {
            kind,
            ty: ret,
            span,
        }
    }

    /// If `expr` is a plain name that refers to an enum (and not to a
    /// variable), that enum.
    fn enum_name(&mut self, expr: &ast::Expr, ctx: &FnCtx) -> Option<EnumId> {
        match self.type_name_of(expr, ctx) {
            Some(Def::Enum(e)) => Some(e),
            _ => None,
        }
    }

    // ---- names ----------------------------------------------------------

    pub(super) fn check_name(
        &mut self,
        ident: &ast::Ident,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        if let Some(id) = ctx.lookup_capturing(&ident.name) {
            return CheckedExpr {
                kind: CheckedExprKind::Local(id),
                ty: ctx.locals[id.0].ty,
                span: ident.span,
            };
        }
        let name = ident.name.as_str();
        if let Some(id) = self.const_named(name) {
            return self.const_value(id, Some(ident.span));
        }
        if self.aliases.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a module, not a value"))
                    .with_code("E0214")
                    .with_primary(ident.span, "modules cannot be used as values")
                    .with_help(format!("use one of its items: `{name}.item`")),
            );
            return error_expr(ident.span);
        }
        if let Some(&(e, v)) = self.prelude_variants.get(name) {
            return self.check_variant(e, v, ident.span, None, ident.span, expected, ctx);
        }
        if let Some(def) = self.type_by_name.get(name).copied() {
            let diag = match def {
                Def::List | Def::Map | Def::Box | Def::Fn | Def::Tuple => {
                    Diagnostic::error(format!("`{name}` is a type, not a value"))
                        .with_code("E0214")
                        .with_primary(ident.span, "types cannot be used as values")
                        .with_help(match def {
                            Def::List => "create a list with `[...]`",
                            Def::Map => {
                                "create a map with `[key: value, ...]`, or `[:]` for an empty one"
                            }
                            _ => "create a box with `Box(value)`",
                        })
                }
                Def::Struct(_) => Diagnostic::error(format!("`{name}` is a struct, not a value"))
                    .with_code("E0214")
                    .with_primary(ident.span, "structs cannot be used as values")
                    .with_help(format!(
                        "to create a `{name}`, write `{name} {{ field: value, ... }}`"
                    )),
                Def::Enum(e) => {
                    let example = self.enums[e.0]
                        .variants
                        .first()
                        .map_or_else(String::new, |v| format!(", such as `{name}.{}`", v.name));
                    Diagnostic::error(format!("`{name}` is an enum, not a value"))
                        .with_code("E0214")
                        .with_primary(ident.span, "enums cannot be used as values")
                        .with_help(format!("use one of its variants{example}"))
                }
            };
            self.diags.push(diag);
        } else if let Some(&id) = self.by_name.get(name) {
            return self.fn_value(id, ident);
        } else if brim_stdlib::lookup(name).is_some()
            || super::strings::intrinsic_function(name).is_some()
        {
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a function, not a value"))
                    .with_code("E0214")
                    .with_primary(ident.span, "built-in functions cannot be used as values")
                    .with_help(format!(
                        "to call it, write `{name}(...)`, or wrap it: `fn(x: i64) {{ {name}(x) }}`"
                    )),
            );
        } else {
            let mut diag = Diagnostic::error(format!("undefined variable `{name}`"))
                .with_code("E0200")
                .with_primary(ident.span, "not found in this scope");
            if let Some(similar) = suggest::similar(name, ctx.visible_names()) {
                diag = diag.with_help(format!(
                    "a variable with a similar name exists: `{similar}`"
                ));
            }
            self.diags.push(diag);
        }
        error_expr(ident.span)
    }

    // ---- generic inference ------------------------------------------------

    /// Checks `args` against parameter types that may use the item's type
    /// parameters, inferring those from `expected` (matched against
    /// `result`) and from the arguments. Returns the checked arguments and
    /// the inferred type arguments.
    fn infer_args(
        &mut self,
        type_param_count: usize,
        params: &[Type],
        result: Type,
        args: &[&ast::Expr],
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> (Vec<CheckedExpr>, Vec<Option<Type>>) {
        let mut bindings: Vec<Option<Type>> = vec![None; type_param_count];
        if let Some(expected) = expected {
            self.types.unify(result, expected, &mut bindings);
        }
        let checked = self.infer_from(&mut bindings, params, args, ctx);
        (checked, bindings)
    }

    /// Checks `args` against `params`, extending `bindings` (see
    /// [`infer_args`](Self::infer_args)).
    fn infer_from(
        &mut self,
        bindings: &mut [Option<Type>],
        params: &[Type],
        args: &[&ast::Expr],
        ctx: &mut FnCtx,
    ) -> Vec<CheckedExpr> {
        // Arguments whose own type may depend on the expected type (`None`,
        // `Ok(x)`, generic calls, ...) are checked last, so the other
        // arguments can determine the type arguments first. Checking order
        // does not change evaluation order.
        let mut checked: Vec<Option<CheckedExpr>> = vec![None; args.len()];
        for deferred in [false, true] {
            for (i, arg) in args.iter().enumerate() {
                if checked[i].is_some() || self.uses_expected(arg, ctx) != deferred {
                    continue;
                }
                let param = params.get(i).copied();
                let hint = param.and_then(|p| self.types.subst_partial(p, bindings));
                let value = self.check_expr_with(arg, hint, ctx);
                if let Some(p) = param {
                    self.types.unify(p, value.ty, bindings);
                }
                checked[i] = Some(value);
            }
        }
        checked
            .into_iter()
            .map(|c| c.expect("every argument is checked"))
            .collect()
    }

    /// Whether checking `expr` may use the type the context expects.
    fn uses_expected(&self, expr: &ast::Expr, ctx: &FnCtx) -> bool {
        match &expr.kind {
            ast::ExprKind::Name(name) => !ctx.is_variable(&name.name),
            ast::ExprKind::Call { .. }
            | ast::ExprKind::MethodCall { .. }
            | ast::ExprKind::Field { .. }
            | ast::ExprKind::StructLit { .. } => true,
            _ => false,
        }
    }

    /// Reports type parameters that could not be inferred. Returns the
    /// complete type arguments, or `None` after an error.
    fn require_inferred(
        &mut self,
        bindings: Vec<Option<Type>>,
        generic: Generic<'_>,
        args: &[CheckedExpr],
        span: Span,
    ) -> Option<Vec<Type>> {
        if bindings.iter().all(Option::is_some) {
            return Some(bindings.into_iter().map(Option::unwrap).collect());
        }
        // A failed argument often explains the missing type; don't pile on.
        if args.iter().all(|a| a.ty != Type::Error) && !self.quiet_inference {
            let missing: Vec<String> = bindings
                .iter()
                .zip(generic.params)
                .filter(|(b, _)| b.is_none())
                .map(|(_, name)| format!("`{name}`"))
                .collect();
            self.diags.push(
                Diagnostic::error(format!(
                    "cannot infer the type {} {} of {}",
                    if missing.len() == 1 {
                        "parameter"
                    } else {
                        "parameters"
                    },
                    missing.join(", "),
                    generic.what
                ))
                .with_code("E0226")
                .with_primary(span, "the type is not known here")
                .with_help("add a type annotation, for example `let x: Option[i64] = None`"),
            );
        }
        None
    }

    /// The type `def[Param(0), Param(1), ...]`: the declaration's own type,
    /// as seen from inside it.
    pub(super) fn own_type(&mut self, def: Def) -> Type {
        let n = self.def_type_params(def).len();
        let params = (0..n)
            .map(|i| Type::Param(u32::try_from(i).expect("few type parameters")))
            .collect();
        self.types.intern(def, params)
    }

    // ---- enum variants ----------------------------------------------------

    /// `Enum.Variant` (`args` is `None`) or `Enum.Variant(args)`.
    fn check_variant_name(
        &mut self,
        e: EnumId,
        variant: &ast::Ident,
        args: Option<&Vec<ast::Expr>>,
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        match self.variant_index(e, variant) {
            Some(v) => {
                let args: Option<Vec<&ast::Expr>> = args.map(|a| a.iter().collect());
                self.check_variant(e, v, variant.span, args.as_deref(), span, expected, ctx)
            }
            None => {
                for arg in args.into_iter().flatten() {
                    self.check_expr(arg, ctx);
                }
                error_expr(span)
            }
        }
    }

    /// The index of `variant` in enum `e`, reporting an unknown variant.
    pub(super) fn variant_index(&mut self, e: EnumId, variant: &ast::Ident) -> Option<usize> {
        let info = &self.enums[e.0];
        if let Some(v) = info.variants.iter().position(|v| v.name == variant.name) {
            return Some(v);
        }
        let mut diag = Diagnostic::error(format!(
            "enum `{}` has no variant `{}`",
            info.name, variant.name
        ))
        .with_code("E0227")
        .with_primary(variant.span, "unknown variant");
        if let Some(span) = info.name_span {
            diag = diag.with_secondary(span, format!("`{}` is declared here", info.name));
        }
        diag = match suggest::similar(&variant.name, info.variants.iter().map(|v| v.name.as_str()))
        {
            Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
            None => {
                let names: Vec<String> = info
                    .variants
                    .iter()
                    .map(|v| format!("`{}`", v.name))
                    .collect();
                diag.with_note(format!("the variants are {}", names.join(", ")))
            }
        };
        self.diags.push(diag);
        None
    }

    /// Constructs variant `v` of enum `e`. `args` is `None` without
    /// parentheses.
    #[allow(clippy::too_many_arguments)]
    fn check_variant(
        &mut self,
        e: EnumId,
        v: usize,
        name_span: Span,
        args: Option<&[&ast::Expr]>,
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let fields = self.enums[e.0].variants[v].fields.clone();
        let variant_name = self.enums[e.0].variants[v].name.clone();
        let given = args.map_or(0, <[_]>::len);
        let count_ok = match args {
            None if !fields.is_empty() => {
                let placeholders = vec!["..."; fields.len()].join(", ");
                self.diags.push(
                    Diagnostic::error(format!(
                        "variant `{variant_name}` needs {}",
                        plural(fields.len(), "value")
                    ))
                    .with_code("E0228")
                    .with_primary(name_span, "missing values")
                    .with_help(format!("write `{variant_name}({placeholders})`")),
                );
                false
            }
            Some(_) if fields.is_empty() => {
                self.diags.push(
                    Diagnostic::error(format!("variant `{variant_name}` takes no values"))
                        .with_code("E0228")
                        .with_primary(span, "")
                        .with_help(format!("write just `{variant_name}`")),
                );
                false
            }
            Some(_) if given != fields.len() => {
                self.diags.push(
                    Diagnostic::error(format!(
                        "variant `{variant_name}` takes {} but {} supplied",
                        plural(fields.len(), "value"),
                        supplied(given)
                    ))
                    .with_code("E0228")
                    .with_primary(span, format!("expected {}", plural(fields.len(), "value"))),
                );
                false
            }
            _ => true,
        };
        let def = Def::Enum(e);
        let result = self.own_type(def);
        let type_params = self.enums[e.0].type_params.clone();
        let args = args.unwrap_or(&[]);
        let (checked, bindings) =
            self.infer_args(type_params.len(), &fields, result, args, expected, ctx);
        if !count_ok {
            return error_expr(span);
        }
        let generic = Generic {
            what: format!("`{variant_name}`"),
            params: &type_params,
        };
        let Some(type_args) = self.require_inferred(bindings, generic, &checked, span) else {
            return error_expr(span);
        };
        for (arg, field) in checked.iter().zip(&fields) {
            let field = self.types.subst(*field, &type_args);
            self.expect_type(arg, field, None, "");
        }
        let ty = self.types.intern(def, type_args);
        CheckedExpr {
            kind: CheckedExprKind::Variant {
                index: v,
                args: checked,
            },
            ty,
            span,
        }
    }

    // ---- structs ------------------------------------------------------------

    /// Looks up `field` in a value of type `ty`: its index and type.
    /// Reports an error (unless `ty` is already an error) when there is none.
    pub(super) fn field(
        &mut self,
        ty: Type,
        field: &ast::Ident,
        base_span: Span,
    ) -> Option<(usize, Type)> {
        if ty == Type::Error {
            return None;
        }
        if let Some(items) = self.tuple_items(ty) {
            if let Ok(index) = field.name.parse::<usize>()
                && index < items.len()
            {
                return Some((index, items[index]));
            }
            let shown = self.show(ty);
            self.diags.push(
                Diagnostic::error(format!("`{shown}` has no field `{}`", field.name))
                    .with_code("E0224")
                    .with_primary(field.span, "unknown field")
                    .with_note(format!(
                        "a tuple of {} values has the fields `0` to `{}`",
                        items.len(),
                        items.len() - 1
                    )),
            );
            return None;
        }
        let Some((id, args)) = self.as_struct(ty) else {
            let shown = self.show(ty);
            let mut diag = Diagnostic::error(format!("`{shown}` has no fields"))
                .with_code("E0224")
                .with_primary(field.span, "unknown field")
                .with_secondary(base_span, format!("this is `{shown}`"))
                .with_note("only structs have fields");
            if self.as_enum(ty).is_some() {
                diag = diag.with_help("use `match` to get the values inside an enum");
            }
            self.diags.push(diag);
            return None;
        };
        let info = &self.structs[id.0];
        if let Some(index) = info.fields.iter().position(|f| f.name == field.name) {
            let field_ty = info.fields[index].ty;
            return Some((index, self.types.subst(field_ty, &args)));
        }
        let mut diag = Diagnostic::error(format!(
            "struct `{}` has no field `{}`",
            info.name, field.name
        ))
        .with_code("E0224")
        .with_primary(field.span, "unknown field")
        .with_secondary(info.name_span, format!("`{}` is declared here", info.name));
        diag = match suggest::similar(&field.name, info.fields.iter().map(|f| f.name.as_str())) {
            Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
            None => {
                let names: Vec<String> = info
                    .fields
                    .iter()
                    .map(|f| format!("`{}`", f.name))
                    .collect();
                if names.is_empty() {
                    diag.with_note(format!("`{}` has no fields", info.name))
                } else {
                    diag.with_note(format!("available fields: {}", names.join(", ")))
                }
            }
        };
        self.diags.push(diag);
        None
    }

    fn check_struct_lit(
        &mut self,
        name: &ast::Ident,
        fields: &[ast::FieldInit],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let qualified = match self.qualified_type(&name.name, name.span) {
            Ok(def) => def,
            Err(()) => {
                for f in fields {
                    self.check_expr(&f.value, ctx);
                }
                return error_expr(span);
            }
        };
        let found = qualified.or_else(|| self.type_by_name.get(name.name.as_str()).copied());
        let id = match found.as_ref() {
            Some(Def::Struct(id)) => Some(*id),
            Some(def @ (Def::List | Def::Map | Def::Box | Def::Fn | Def::Tuple)) => {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is not a struct", name.name))
                        .with_code("E0212")
                        .with_primary(name.span, "built-in type")
                        .with_help(match def {
                            Def::List => "create a list with `[...]`",
                            Def::Map => "create a map with `[key: value, ...]`",
                            _ => "create a box with `Box(value)`",
                        }),
                );
                None
            }
            Some(Def::Enum(_)) => {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is an enum, not a struct", name.name))
                        .with_code("E0212")
                        .with_primary(name.span, "enums have no fields")
                        .with_help(format!("create a value with `{}.Variant(...)`", name.name)),
                );
                None
            }
            None if Type::from_name(&name.name).is_some() => {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is not a struct", name.name))
                        .with_code("E0212")
                        .with_primary(name.span, "built-in type"),
                );
                None
            }
            None => {
                self.unknown_type(&name.name, name.span);
                None
            }
        };
        let Some(id) = id else {
            for f in fields {
                self.check_expr(&f.value, ctx);
            }
            return error_expr(span);
        };

        // Resolve field names first; values are checked in source order.
        let mut ok = true;
        let mut given: Vec<Option<Span>> = vec![None; self.structs[id.0].fields.len()];
        let mut indices: Vec<Option<usize>> = Vec::with_capacity(fields.len());
        let info_fields: Vec<(String, Type, Span)> = self.structs[id.0]
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty, f.span))
            .collect();
        let own = self.own_type(Def::Struct(id));
        for field in fields {
            let Some(index) = info_fields.iter().position(|f| f.0 == field.name.name) else {
                // Reports the unknown field.
                self.field(own, &field.name, name.span);
                ok = false;
                indices.push(None);
                continue;
            };
            if let Some(first) = given[index] {
                self.diags.push(
                    Diagnostic::error(format!(
                        "field `{}` is given more than once",
                        field.name.name
                    ))
                    .with_code("E0223")
                    .with_primary(field.name.span, "given again here")
                    .with_secondary(first, "first given here"),
                );
                ok = false;
                indices.push(None);
                continue;
            }
            given[index] = Some(field.name.span);
            indices.push(Some(index));
        }

        // Fields that could not be resolved are checked without a type.
        let params: Vec<Type> = indices
            .iter()
            .map(|i| i.map_or(Type::Error, |i| info_fields[i].1))
            .collect();
        let values: Vec<&ast::Expr> = fields.iter().map(|f| &f.value).collect();
        let type_params = self.structs[id.0].type_params.clone();
        let (checked, bindings) =
            self.infer_args(type_params.len(), &params, own, &values, expected, ctx);

        let info = &self.structs[id.0];
        let missing: Vec<String> = info
            .fields
            .iter()
            .zip(&given)
            .filter(|(_, g)| g.is_none())
            .map(|(f, _)| format!("`{}`", f.name))
            .collect();
        if !missing.is_empty() {
            let (noun, list) = if missing.len() == 1 {
                ("field", missing[0].clone())
            } else {
                let (last, rest) = missing.split_last().expect("not empty");
                ("fields", format!("{} and {last}", rest.join(", ")))
            };
            self.diags.push(
                Diagnostic::error(format!("missing {noun} {list} in `{}`", info.name))
                    .with_code("E0222")
                    .with_primary(
                        span,
                        format!("every field of `{}` needs a value", info.name),
                    )
                    .with_secondary(info.name_span, format!("`{}` is declared here", info.name)),
            );
            ok = false;
        }
        if !ok {
            return error_expr(span);
        }
        let generic = Generic {
            what: format!("struct `{}`", name.name),
            params: &type_params,
        };
        let Some(type_args) = self.require_inferred(bindings, generic, &checked, span) else {
            return error_expr(span);
        };
        let mut out = Vec::with_capacity(checked.len());
        for (value, index) in checked.into_iter().zip(indices) {
            let index = index.expect("all fields resolved");
            let (_, field_ty, field_span) = info_fields[index];
            let field_ty = self.types.subst(field_ty, &type_args);
            self.expect_type(&value, field_ty, Some(field_span), "field declared here");
            out.push((index, value));
        }
        let ty = self.types.intern(Def::Struct(id), type_args);
        CheckedExpr {
            kind: CheckedExprKind::StructLit { fields: out },
            ty,
            span,
        }
    }

    // ---- operators ------------------------------------------------------------

    fn int_out_of_range(&mut self, value: u64, negative: bool, ty: Type, span: Span) {
        let sign = if negative { "-" } else { "" };
        let (min, max) = match ty {
            Type::U64 => ("0".to_string(), u64::MAX.to_string()),
            ty => {
                let (min, max) = ty.int_range().unwrap_or((i64::MIN, i64::MAX));
                (min.to_string(), max.to_string())
            }
        };
        self.diags.push(
            Diagnostic::error(format!(
                "integer literal `{sign}{value}` is out of range for `{ty}`"
            ))
            .with_code("E0215")
            .with_primary(span, "")
            .with_note(format!("`{ty}` values range from {min} to {max}")),
        );
    }

    /// An integer literal checked with the type it got from the context
    /// (`I64` by default) changed to `ty`, when its value fits: `x + 1`
    /// with `x: u8`. `false` if `expr` is not such a literal.
    pub(super) fn retype_int_literal(&mut self, expr: &mut CheckedExpr, ty: Type) -> bool {
        let (magnitude, negative) = match &expr.kind {
            CheckedExprKind::Int(v) => (v.unsigned_abs(), *v < 0),
            CheckedExprKind::Unary {
                op: UnaryOp::IntNeg,
                operand,
            } => match operand.kind {
                CheckedExprKind::Int(v) if v >= 0 => (v.unsigned_abs(), true),
                _ => return false,
            },
            _ => return false,
        };
        if expr.ty != Type::I64 {
            return false;
        }
        let Some(v) = int_literal(magnitude, negative, ty) else {
            return false;
        };
        *expr = CheckedExpr {
            kind: CheckedExprKind::Int(v),
            ty,
            span: expr.span,
        };
        true
    }

    fn check_unary(
        &mut self,
        op: ast::UnaryOp,
        operand: &ast::Expr,
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let int_expected = expected.filter(|t| t.is_int());
        // A negative literal is one value: `-9223372036854775808` is
        // i64::MIN, even though the literal alone does not fit in an i64.
        if op == ast::UnaryOp::Neg
            && let ast::ExprKind::Int(value) = operand.kind
            && (int_expected.is_some() || value == i64::MIN.unsigned_abs())
        {
            let ty = int_expected.unwrap_or(Type::I64);
            let Some(v) = int_literal(value, true, ty) else {
                self.int_out_of_range(value, true, ty, span);
                return error_expr(span);
            };
            return CheckedExpr {
                kind: CheckedExprKind::Int(v),
                ty,
                span,
            };
        }
        let operand = self.check_expr_with(operand, int_expected, ctx);
        let (op, ty) = match (op, operand.ty) {
            (_, Type::Error) => return error_expr(span),
            (ast::UnaryOp::Neg, ty @ (Type::I64 | Type::I32)) => (UnaryOp::IntNeg, ty),
            (ast::UnaryOp::Neg, Type::F64) => (UnaryOp::FloatNeg, Type::F64),
            (ast::UnaryOp::Not, Type::Bool) => (UnaryOp::BoolNot, Type::Bool),
            (op, ty) => {
                let ty = self.show(ty);
                let expected = if op == ast::UnaryOp::Neg {
                    "i64`, `i32` or `f64"
                } else {
                    "bool"
                };
                self.diags.push(
                    Diagnostic::error(format!(
                        "cannot apply unary `{}` to a value of type `{ty}`",
                        op.as_str()
                    ))
                    .with_code("E0209")
                    .with_primary(operand.span, format!("this is `{ty}`"))
                    .with_note(
                        if matches!(operand.ty, Type::U8 | Type::U64) {
                            format!("`{ty}` is unsigned: it has no negative values")
                        } else {
                            format!("unary `{}` requires `{expected}`", op.as_str())
                        },
                    ),
                );
                return error_expr(span);
            }
        };
        CheckedExpr {
            kind: CheckedExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            ty,
            span,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        use ast::BinaryOp as Op;
        let arithmetic = matches!(op, Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem);
        // `let x: u8 = 1 + 2`: the literals are `u8`s.
        let lhs_expected = expected.filter(|t| arithmetic && t.is_int());
        let lhs = self.check_expr_with(lhs, lhs_expected, ctx);
        // `o == None` and `x + 1`: the left side tells what the right side
        // is.
        let rhs_expected = match op {
            Op::Eq | Op::Ne => Some(lhs.ty),
            Op::And | Op::Or => None,
            _ => lhs.ty.is_int().then_some(lhs.ty),
        };
        let rhs = self.check_expr_with(rhs, rhs_expected, ctx);
        self.resolve_binary(op, op_span, lhs, rhs, span)
    }

    /// Selects the type-specific operation for `lhs op rhs`.
    pub(super) fn resolve_binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: CheckedExpr,
        rhs: CheckedExpr,
        span: Span,
    ) -> CheckedExpr {
        use Type::{Bool, F64, String};
        use ast::BinaryOp as Op;

        if lhs.ty == Type::Error || rhs.ty == Type::Error {
            return error_expr(span);
        }
        // `1 + x` with `x: u8`: the literal becomes a `u8`.
        let (mut lhs, mut rhs) = (lhs, rhs);
        if lhs.ty != rhs.ty && lhs.ty.is_int() && rhs.ty.is_int() {
            let rhs_ty = rhs.ty;
            if !self.retype_int_literal(&mut lhs, rhs_ty) {
                let lhs_ty = lhs.ty;
                self.retype_int_literal(&mut rhs, lhs_ty);
            }
        }
        let int = lhs.ty == rhs.ty && lhs.ty.is_int();
        let resolved = match (op, lhs.ty, rhs.ty) {
            (Op::Add, ty, _) if int => Some((BinaryOp::IntAdd, ty)),
            (Op::Sub, ty, _) if int => Some((BinaryOp::IntSub, ty)),
            (Op::Mul, ty, _) if int => Some((BinaryOp::IntMul, ty)),
            (Op::Div, ty, _) if int => Some((BinaryOp::IntDiv, ty)),
            (Op::Rem, ty, _) if int => Some((BinaryOp::IntRem, ty)),
            (Op::Eq, _, _) if int => Some((BinaryOp::IntEq, Bool)),
            (Op::Ne, _, _) if int => Some((BinaryOp::IntNe, Bool)),
            (Op::Lt, _, _) if int => Some((BinaryOp::IntLt, Bool)),
            (Op::Le, _, _) if int => Some((BinaryOp::IntLe, Bool)),
            (Op::Gt, _, _) if int => Some((BinaryOp::IntGt, Bool)),
            (Op::Ge, _, _) if int => Some((BinaryOp::IntGe, Bool)),
            (Op::Add, String, String) => Some((BinaryOp::StrConcat, String)),
            (Op::Eq, Bool, Bool) => Some((BinaryOp::BoolEq, Bool)),
            (Op::Ne, Bool, Bool) => Some((BinaryOp::BoolNe, Bool)),
            (Op::Eq, String, String) => Some((BinaryOp::StrEq, Bool)),
            (Op::Ne, String, String) => Some((BinaryOp::StrNe, Bool)),
            (Op::Lt, String, String) => Some((BinaryOp::StrLt, Bool)),
            (Op::Le, String, String) => Some((BinaryOp::StrLe, Bool)),
            (Op::Gt, String, String) => Some((BinaryOp::StrGt, Bool)),
            (Op::Ge, String, String) => Some((BinaryOp::StrGe, Bool)),
            (Op::Add, F64, F64) => Some((BinaryOp::FloatAdd, F64)),
            (Op::Sub, F64, F64) => Some((BinaryOp::FloatSub, F64)),
            (Op::Mul, F64, F64) => Some((BinaryOp::FloatMul, F64)),
            (Op::Div, F64, F64) => Some((BinaryOp::FloatDiv, F64)),
            (Op::Rem, F64, F64) => Some((BinaryOp::FloatRem, F64)),
            (Op::Eq, F64, F64) => Some((BinaryOp::FloatEq, Bool)),
            (Op::Ne, F64, F64) => Some((BinaryOp::FloatNe, Bool)),
            (Op::Lt, F64, F64) => Some((BinaryOp::FloatLt, Bool)),
            (Op::Le, F64, F64) => Some((BinaryOp::FloatLe, Bool)),
            (Op::Gt, F64, F64) => Some((BinaryOp::FloatGt, Bool)),
            (Op::Ge, F64, F64) => Some((BinaryOp::FloatGe, Bool)),
            (Op::And | Op::Or, Bool, Bool) => None,
            (Op::Eq | Op::Ne, a, b)
                if a == b
                    && matches!(a, Type::Named(_) | Type::Param(_))
                    && self.not_comparable(a, super::traits::EQ).is_none() =>
            {
                let op = if op == Op::Eq {
                    BinaryOp::ValueEq
                } else {
                    BinaryOp::ValueNe
                };
                Some((op, Bool))
            }
            _ => {
                self.invalid_binary(op, op_span, &lhs, &rhs);
                return error_expr(span);
            }
        };
        let (kind, ty) = match resolved {
            Some((op, ty)) => (
                CheckedExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                ty,
            ),
            None if op == Op::And => (CheckedExprKind::And(Box::new(lhs), Box::new(rhs)), Bool),
            None => (CheckedExprKind::Or(Box::new(lhs), Box::new(rhs)), Bool),
        };
        CheckedExpr { kind, ty, span }
    }

    fn invalid_binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: &CheckedExpr,
        rhs: &CheckedExpr,
    ) {
        use ast::BinaryOp as Op;
        let sym = op.as_str();
        let (l, r) = (self.show(lhs.ty), self.show(rhs.ty));
        let named = matches!(lhs.ty, Type::Named(_)) && lhs.ty == rhs.ty;
        let param = matches!(lhs.ty, Type::Param(_)) && lhs.ty == rhs.ty;
        let mut diag = Diagnostic::error(format!("cannot apply `{sym}` to `{l}` and `{r}`"))
            .with_code("E0209")
            .with_primary(op_span, format!("`{l} {sym} {r}` is not defined"))
            .with_secondary(lhs.span, format!("`{l}`"))
            .with_secondary(rhs.span, format!("`{r}`"));
        let inner = if named {
            self.not_comparable(lhs.ty, super::traits::EQ)
        } else {
            None
        };
        diag = match (op, inner) {
            (Op::Eq | Op::Ne, Some(inner @ Type::Param(_))) => {
                let inner = self.show(inner);
                diag.with_note(format!(
                    "`{l}` holds a `{inner}`, which can be any type, and not every type can be compared with `{sym}`"
                ))
            }
            (Op::Eq | Op::Ne, Some(inner)) if inner == lhs.ty => {
                diag.with_note(format!("functions cannot be compared with `{sym}`"))
            }
            (Op::Eq | Op::Ne, Some(inner)) => {
                let inner = self.show(inner);
                diag.with_note(format!(
                    "`{l}` holds a `{inner}`, which cannot be compared with `{sym}`"
                ))
            }
            (op, _) => match op {
            Op::Eq | Op::Ne if param => diag
                .with_note(format!(
                    "`{l}` can be any type, and not every type can be compared with `{sym}`"
                ))
                .with_help(format!("add a bound to the type parameter: `{l}: Eq`")),
            Op::Add if lhs.ty == Type::String || rhs.ty == Type::String => diag
                .with_note("`+` joins two strings or adds two numbers of the same type")
                .with_note("Brim never converts values to strings implicitly"),
            _ if matches!(
                (lhs.ty, rhs.ty),
                (Type::I64, Type::F64) | (Type::F64, Type::I64)
            ) =>
            {
                diag.with_note("Brim never converts between `i64` and `f64` implicitly")
                    .with_help(
                        "convert the `i64` with `to_float(...)`, or the `f64` with `to_int(...)`",
                    )
            }
            _ if lhs.ty.is_int() && rhs.ty.is_int() => {
                let to = match lhs.ty {
                    Type::I32 => "to_i32",
                    Type::U8 => "to_u8",
                    Type::U64 => "to_u64",
                    _ => "to_int",
                };
                diag.with_note("Brim never converts between integer types implicitly")
                    .with_help(format!("convert one side, for example with `{to}(...)`"))
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => diag.with_note(format!(
                "`{sym}` requires two integers of the same type or two `f64` operands"
            )),
            Op::Lt | Op::Le | Op::Gt | Op::Ge => diag.with_note(format!(
                "`{sym}` compares two numbers of the same type or two `string` values"
            )),
            Op::Eq | Op::Ne => diag.with_note(format!("`{sym}` compares two values of the same type")),
            Op::And | Op::Or => diag.with_note(format!("`{sym}` requires two `bool` operands")),
            },
        };
        self.diags.push(diag);
    }

    /// `f"text {value} text"`: the pieces joined, each value shown as
    /// `print` shows it (strings as they are).
    fn check_fstring(
        &mut self,
        parts: &[ast::FStringPart],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let mut pieces = Vec::with_capacity(parts.len());
        let mut failed = false;
        for part in parts {
            let piece = match part {
                ast::FStringPart::Text(text) => CheckedExpr {
                    kind: CheckedExprKind::Str(text.clone()),
                    ty: Type::String,
                    span,
                },
                ast::FStringPart::Value(value) => {
                    let value = self.check_expr(value, ctx);
                    match value.ty {
                        Type::Error => {
                            failed = true;
                            continue;
                        }
                        Type::String => value,
                        ty if ty != Type::Void
                            && self.not_comparable(ty, super::traits::SHOW).is_none() =>
                        {
                            CheckedExpr {
                                span: value.span,
                                ty: Type::String,
                                kind: CheckedExprKind::Show(Box::new(value)),
                            }
                        }
                        Type::Void => {
                            self.diags.push(
                                Diagnostic::error("this value cannot be shown in an f-string")
                                    .with_code("E0206")
                                    .with_primary(
                                        value.span,
                                        "calls a function that returns nothing",
                                    ),
                            );
                            failed = true;
                            continue;
                        }
                        ty => {
                            let shown = self.show(ty);
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "a `{shown}` cannot be shown in an f-string"
                                ))
                                .with_code("E0206")
                                .with_primary(value.span, "no text form")
                                .with_note(
                                    "f-strings show numbers, booleans, strings, and structs, enums, lists and maps of them",
                                ),
                            );
                            failed = true;
                            continue;
                        }
                    }
                }
            };
            pieces.push(piece);
        }
        if failed {
            return error_expr(span);
        }
        let mut pieces = pieces.into_iter();
        let first = pieces.next().unwrap_or(CheckedExpr {
            kind: CheckedExprKind::Str(String::new()),
            ty: Type::String,
            span,
        });
        pieces.fold(first, |joined, piece| CheckedExpr {
            kind: CheckedExprKind::Binary {
                op: BinaryOp::StrConcat,
                lhs: Box::new(joined),
                rhs: Box::new(piece),
            },
            ty: Type::String,
            span,
        })
    }

    /// The element types of a tuple type.
    pub(super) fn tuple_items(&self, ty: Type) -> Option<Vec<Type>> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        (named.def == Def::Tuple).then(|| named.args.clone())
    }

    /// `(a, b, ...)`.
    fn check_tuple(
        &mut self,
        items: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let expected_items = expected
            .and_then(|t| self.tuple_items(t))
            .filter(|t| t.len() == items.len());
        let mut fields = Vec::with_capacity(items.len());
        let mut types = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let want = expected_items.as_ref().map(|t| t[i]);
            let value = self.check_expr_with(item, want, ctx);
            if value.ty == Type::Void {
                self.diags.push(
                    Diagnostic::error("a tuple cannot hold a value of a call that returns nothing")
                        .with_code("E0213")
                        .with_primary(value.span, ""),
                );
                return error_expr(span);
            }
            types.push(value.ty);
            fields.push((i, value));
        }
        if types.contains(&Type::Error) {
            return error_expr(span);
        }
        CheckedExpr {
            kind: CheckedExprKind::StructLit { fields },
            ty: self.types.intern(Def::Tuple, types),
            span,
        }
    }

    /// The parameter and return types of a function type.
    pub(super) fn fn_type_parts(&self, ty: Type) -> Option<(Vec<Type>, Type)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        if named.def != Def::Fn {
            return None;
        }
        let (ret, params) = named.args.split_last().expect("a return type");
        Some((params.to_vec(), *ret))
    }

    /// A named function used as a value.
    fn fn_value(&mut self, id: FnId, ident: &ast::Ident) -> CheckedExpr {
        let sig = &self.sigs[id.0];
        if !sig.type_params.is_empty() {
            self.diags.push(
                Diagnostic::error(format!(
                    "generic function `{}` cannot be used as a value",
                    ident.name
                ))
                .with_code("E0214")
                .with_primary(ident.span, "its type arguments are not known here")
                .with_help("wrap it in a `fn` expression that calls it with concrete types"),
            );
            return error_expr(ident.span);
        }
        let mut args: Vec<Type> = sig.params.iter().map(|p| p.ty).collect();
        args.push(sig.ret);
        CheckedExpr {
            kind: CheckedExprKind::FnRef(id),
            ty: self.types.intern(Def::Fn, args),
            span: ident.span,
        }
    }

    /// `callee(args)` where `callee` is a function value.
    pub(super) fn call_value(
        &mut self,
        callee: CheckedExpr,
        name: &str,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let Some((params, ret)) = self.fn_type_parts(callee.ty) else {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            return error_expr(span);
        };
        let Some(args) = self.fixed_args(name, &params, args, span, ctx) else {
            return error_expr(span);
        };
        CheckedExpr {
            kind: CheckedExprKind::CallValue {
                callee: Box::new(callee),
                args,
            },
            ty: ret,
            span,
        }
    }

    /// `fn(params) -> ret { body }`: checked like a function, inside the
    /// one around it, whose variables it copies when it uses them.
    pub(super) fn check_lambda(
        &mut self,
        params: &[ast::Param],
        ret: Option<&ast::TypeExpr>,
        body: &ast::Block,
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let param_types: Vec<Type> = params
            .iter()
            .map(|p| self.resolve_type(&p.ty, "a parameter"))
            .collect();
        let ret_ty = match ret {
            Some(ret) => self.resolve_type(ret, "return"),
            None => Type::Void,
        };
        // The function gets its id now; its body is filled in below, so
        // `fn` expressions inside it get later ids.
        let id = FnId(self.fn_decls.len() + self.lambdas.len());
        self.lambdas.push(None);

        // Scope 0 holds captured variables, scope 1 the parameters and the
        // body's top-level statements.
        let mut inner = FnCtx {
            scopes: vec![HashMap::new(), HashMap::new()],
            ret: ret_ty,
            ret_span: ret.map(ast::TypeExpr::span),
            ..FnCtx::default()
        };
        // Local 0: the environment the captured values arrive in.
        inner.locals.push(Local {
            name: "<environment>".into(),
            ty: Type::Void,
            mutable: false,
            span,
        });
        inner.reassigned.push(false);
        for (param, ty) in params.iter().zip(&param_types) {
            if inner.scopes[1].contains_key(&param.name.name) {
                self.diags.push(
                    Diagnostic::error(format!("duplicate parameter `{}`", param.name.name))
                        .with_code("E0204")
                        .with_primary(param.name.span, ""),
                );
                continue;
            }
            let local = LocalId(inner.locals.len());
            inner.locals.push(Local {
                name: param.name.name.clone(),
                ty: *ty,
                mutable: false,
                span: param.name.span,
            });
            inner.reassigned.push(false);
            inner.scopes[1].insert(param.name.name.clone(), local);
        }
        let param_count = inner.locals.len();

        inner.parent = Some(Box::new(std::mem::take(ctx)));
        let (checked_body, diverges) = self.check_stmts(&body.stmts, &mut inner);
        *ctx = *inner.parent.take().expect("the parent is restored once");

        if !diverges && !matches!(ret_ty, Type::Void | Type::Error) {
            let close = Span::new(body.span.end.saturating_sub(1), body.span.end);
            let shown = self.show(ret_ty);
            self.diags.push(
                Diagnostic::error("`fn` expression may end without returning a value")
                    .with_code("E0207")
                    .with_primary(close, "missing `return` before the end of the function")
                    .with_note(format!("its return type is `{shown}`")),
            );
        }
        for (local, reassigned) in inner.locals.iter().zip(&inner.reassigned) {
            if local.mutable && !reassigned {
                self.diags.push(
                    Diagnostic::warning(format!(
                        "variable `{}` is declared `mut` but never reassigned",
                        local.name
                    ))
                    .with_code("W0003")
                    .with_primary(local.span, "")
                    .with_help("remove `mut`"),
                );
            }
        }

        let (outer, own): (Vec<LocalId>, Vec<LocalId>) = inner.captures.iter().copied().unzip();
        let enclosing = self.current_fn_name.clone();
        self.lambdas[id.0 - self.fn_decls.len()] = Some(CheckedFn {
            name: format!("{enclosing}.<fn>"),
            mut_self: false,
            span,
            type_params: self.type_params.clone(),
            param_count,
            ret: ret_ty,
            locals: inner.locals,
            body: checked_body,
            captures: Some(own),
        });
        let mut fn_args = param_types;
        fn_args.push(ret_ty);
        let ty = if fn_args.contains(&Type::Error) {
            Type::Error
        } else {
            self.types.intern(Def::Fn, fn_args)
        };
        CheckedExpr {
            kind: CheckedExprKind::Closure {
                func: id,
                captures: outer,
            },
            ty,
            span,
        }
    }

    /// `to_i32(x)`, `to_u8(x)` or `to_u64(x)`.
    fn check_int_cast(
        &mut self,
        name: &str,
        target: Type,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let args: Vec<CheckedExpr> = args.iter().map(|a| self.check_expr(a, ctx)).collect();
        let [value] = <[CheckedExpr; 1]>::try_from(args).unwrap_or_else(|args| {
            self.diags.push(
                Diagnostic::error(format!(
                    "`{name}` takes 1 argument but {} supplied",
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(span, "expected 1 argument"),
            );
            [error_expr(span)]
        });
        if value.ty == Type::Error {
            return error_expr(span);
        }
        if !value.ty.is_int() {
            let ty = self.show(value.ty);
            let mut diag = Diagnostic::error(format!("`{name}` cannot be called with `{ty}`"))
                .with_code("E0206")
                .with_primary(value.span, "unsupported argument type")
                .with_note(format!("`{name}` converts an integer of another type"));
            if value.ty == Type::F64 {
                diag = diag.with_help(format!("convert the `f64` with `{name}(to_int(x))`"));
            }
            self.diags.push(diag);
            return error_expr(span);
        }
        CheckedExpr {
            kind: CheckedExprKind::IntCast(Box::new(value)),
            ty: target,
            span,
        }
    }

    /// Builtins whose argument is a `u64`, which has its own runtime
    /// functions, and `to_int` of an integer.
    fn int_builtin(
        &mut self,
        name: &str,
        args: &mut Vec<CheckedExpr>,
        span: Span,
    ) -> Option<CheckedExpr> {
        use brim_runtime::RuntimeFn as R;
        // `assert_eq(x, 5)` with `x: u8`: the literal becomes a `u8`.
        if let [a, b] = args.as_mut_slice()
            && a.ty != b.ty
            && a.ty.is_int()
            && b.ty.is_int()
            && !self.retype_int_literal(b, a.ty)
        {
            self.retype_int_literal(a, b.ty);
        }
        let builtin = |runtime, args, ty| CheckedExpr {
            kind: CheckedExprKind::Builtin { runtime, args },
            ty,
            span,
        };
        match (name, args.as_slice()) {
            ("to_int", [arg]) if arg.ty.is_int() => Some(CheckedExpr {
                kind: CheckedExprKind::IntCast(Box::new(args.pop().expect("one"))),
                ty: Type::I64,
                span,
            }),
            ("print", [arg]) if arg.ty == Type::U64 => {
                Some(builtin(R::PrintU64, std::mem::take(args), Type::Void))
            }
            ("to_string", [arg]) if arg.ty == Type::U64 => {
                Some(builtin(R::U64ToStr, std::mem::take(args), Type::String))
            }
            ("to_float", [arg]) if arg.ty == Type::U64 => {
                Some(builtin(R::U64ToF64, std::mem::take(args), Type::F64))
            }
            ("assert_eq", [a, b]) if a.ty == Type::U64 && b.ty == Type::U64 => {
                // Compared as text, which also shows the values correctly.
                let texts = std::mem::take(args)
                    .into_iter()
                    .map(|a| {
                        let span = a.span;
                        CheckedExpr {
                            kind: CheckedExprKind::Builtin {
                                runtime: R::U64ToStr,
                                args: vec![a],
                            },
                            ty: Type::String,
                            span,
                        }
                    })
                    .collect();
                Some(builtin(R::AssertEqStr, texts, Type::Void))
            }
            _ => None,
        }
    }

    /// `print(value)` or `to_string(value)` for a struct, enum, list or map.
    fn show_value(&mut self, name: &str, mut args: Vec<CheckedExpr>, span: Span) -> CheckedExpr {
        let value = args.pop().expect("one argument");
        if let Some(inner) = self.not_comparable(value.ty, super::traits::SHOW) {
            let ty = self.show(value.ty);
            let why = if matches!(inner, Type::Param(_)) {
                "which can be any type, and not every type has a text form"
            } else {
                "which has no text form"
            };
            let note = if inner == value.ty {
                "functions have no text form".to_string()
            } else {
                format!("`{ty}` holds a `{}`, {why}", self.show(inner))
            };
            self.diags.push(
                Diagnostic::error(format!("`{name}` cannot show a `{ty}`"))
                    .with_code("E0206")
                    .with_primary(value.span, "unsupported argument type")
                    .with_note(note),
            );
            return error_expr(span);
        }
        let shown = CheckedExpr {
            span: value.span,
            ty: Type::String,
            kind: CheckedExprKind::Show(Box::new(value)),
        };
        if name == "to_string" {
            return CheckedExpr { span, ..shown };
        }
        CheckedExpr {
            kind: CheckedExprKind::Builtin {
                runtime: brim_runtime::RuntimeFn::PrintStr,
                args: vec![shown],
            },
            ty: Type::Void,
            span,
        }
    }

    // ---- calls ----------------------------------------------------------------

    fn check_call(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let name = callee.name.as_str();
        let arg_refs: Vec<&ast::Expr> = args.iter().collect();

        if let Some(local) = ctx.lookup_capturing(name) {
            let ty = ctx.locals[local.0].ty;
            if self.fn_type_parts(ty).is_some() || ty == Type::Error {
                let callee = CheckedExpr {
                    kind: CheckedExprKind::Local(local),
                    ty,
                    span: callee.span,
                };
                return self.call_value(callee, name, args, span, ctx);
            }
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let shown = self.show(ty);
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a variable, not a function"))
                    .with_code("E0214")
                    .with_primary(callee.span, "cannot be called")
                    .with_secondary(
                        ctx.locals[local.0].span,
                        format!("`{name}` is declared here"),
                    )
                    .with_note(format!(
                        "`{name}` is a `{shown}`; only function values (`fn(...)`) can be called"
                    )),
            );
            return error_expr(span);
        }

        if let Some(&id) = self.by_name.get(name) {
            return self.finish_call(id, None, &arg_refs, span, expected, ctx);
        }
        if self.type_by_name.get(name) == Some(&Def::Box) {
            return self.check_box_new(callee, args, span, expected, ctx);
        }
        let int_target = match name {
            "to_i32" => Some(Type::I32),
            "to_u8" => Some(Type::U8),
            "to_u64" => Some(Type::U64),
            _ => None,
        };
        if let Some(target) = int_target {
            return self.check_int_cast(name, target, args, span, ctx);
        }
        if let Some(result) = self.check_intrinsic_call(callee, args, span, ctx) {
            return result;
        }

        // `__` builtins are the standard library modules' own primitives.
        let builtin = brim_stdlib::lookup(name)
            .filter(|b| !brim_stdlib::is_internal(b.name) || self.in_std_module());
        if let Some(builtin) = builtin {
            let mut args: Vec<CheckedExpr> = args.iter().map(|a| self.check_expr(a, ctx)).collect();
            let arities = builtin.arities();
            if !arities.contains(&args.len()) {
                let expected = arities
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(" or ");
                let noun = if arities == [1] {
                    "argument"
                } else {
                    "arguments"
                };
                self.diags.push(
                    Diagnostic::error(format!(
                        "`{name}` takes {expected} {noun} but {} supplied",
                        supplied(args.len())
                    ))
                    .with_code("E0205")
                    .with_primary(span, format!("expected {expected} {noun}")),
                );
                return error_expr(span);
            }
            if args.iter().any(|a| a.ty == Type::Error) {
                return error_expr(span);
            }
            if let ("print" | "to_string", [arg]) = (name, args.as_slice())
                && matches!(arg.ty, Type::Named(_) | Type::Param(_))
            {
                return self.show_value(name, args, span);
            }
            if let Some(result) = self.int_builtin(name, &mut args, span) {
                return result;
            }
            // `i32` and `u8` values are held as `i64`: they use the `i64`
            // forms of the builtins.
            let prims = args
                .iter()
                .map(|a| match a.ty {
                    Type::I32 | Type::U8 => Some(Prim::I64),
                    ty => ty.to_prim(),
                })
                .collect::<Option<Vec<_>>>();
            let overload = prims.as_deref().and_then(|p| builtin.resolve(p));
            let Some(overload) = overload else {
                let found = args
                    .iter()
                    .map(|a| format!("`{}`", self.show(a.ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let accepted = builtin
                    .overloads
                    .iter()
                    .map(|o| {
                        let params: Vec<_> = o.params.iter().map(|p| p.to_string()).collect();
                        format!("`{name}({})`", params.join(", "))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let arg_span = args
                    .first()
                    .zip(args.last())
                    .map_or(span, |(a, b)| a.span.to(b.span));
                self.diags.push(
                    Diagnostic::error(format!("`{name}` cannot be called with {found}"))
                        .with_code("E0206")
                        .with_primary(arg_span, "unsupported argument type")
                        .with_note(format!("available forms: {accepted}")),
                );
                return error_expr(span);
            };
            return CheckedExpr {
                kind: CheckedExprKind::Builtin {
                    runtime: overload.runtime,
                    args,
                },
                ty: Type::from_prim(overload.ret),
                span,
            };
        }

        if let Some(&(e, v)) = self.prelude_variants.get(name) {
            return self.check_variant(e, v, callee.span, Some(&arg_refs), span, expected, ctx);
        }

        for arg in args {
            self.check_expr(arg, ctx);
        }
        let mut diag = Diagnostic::error(format!("undefined function `{name}`"))
            .with_code("E0201")
            .with_primary(callee.span, "not found");
        match self.type_by_name.get(name) {
            Some(Def::List | Def::Map | Def::Box | Def::Fn | Def::Tuple) => {
                diag = diag.with_help("create a list with `[...]`, or a map with `[key: value]`");
            }
            Some(Def::Struct(_)) => {
                diag = diag.with_help(format!(
                    "`{name}` is a struct; create one with `{name} {{ field: value, ... }}`"
                ));
            }
            Some(Def::Enum(_)) => {
                diag = diag.with_help(format!(
                    "`{name}` is an enum; create a value with `{name}.Variant(...)`"
                ));
            }
            None => {
                let candidates = self
                    .by_name
                    .keys()
                    .copied()
                    .chain(
                        brim_stdlib::BUILTINS
                            .iter()
                            .map(|b| b.name)
                            .filter(|n| !brim_stdlib::is_internal(n)),
                    )
                    .chain(super::strings::INTRINSIC_FUNCTIONS.iter().copied());
                if let Some(similar) = suggest::similar(name, candidates) {
                    diag = diag.with_help(format!(
                        "a function with a similar name exists: `{similar}`"
                    ));
                }
            }
        }
        self.diags.push(diag);
        error_expr(span)
    }
}

/// The value of an integer literal of magnitude `magnitude` (negated if
/// `negative`) as a `ty`, held in an `i64` (`u64` keeps its bits); `None`
/// if it does not fit.
pub(super) fn int_literal(magnitude: u64, negative: bool, ty: Type) -> Option<i64> {
    let value = if negative {
        -i128::from(magnitude)
    } else {
        i128::from(magnitude)
    };
    let (min, max) = match ty {
        Type::U64 => (0, i128::from(u64::MAX)),
        Type::I64 => (i128::from(i64::MIN), i128::from(i64::MAX)),
        ty => {
            let (min, max) = ty.int_range()?;
            (i128::from(min), i128::from(max))
        }
    };
    (min..=max).contains(&value).then_some(value as u64 as i64)
}
