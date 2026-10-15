//! Expressions, including generic calls, struct literals and enum variants.
//!
//! Type arguments are inferred: first from the type the context expects
//! (`let x: Option[i64] = None`), then from the arguments, left to right.
//! Each argument is checked with the type its parameter has once the type
//! arguments known so far are filled in, so `wrap(None)` works when `wrap`
//! takes an `Option[i64]`.

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx, error_expr, plural, supplied};
use crate::checked::{BinaryOp, CheckedExpr, CheckedExprKind, CheckedPlace, FnId, UnaryOp};
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
            ast::ExprKind::Int(value) => match i64::try_from(*value) {
                Ok(v) => (CheckedExprKind::Int(v), Type::I64),
                Err(_) => {
                    self.int_out_of_range(*value, span);
                    return error_expr(span);
                }
            },
            ast::ExprKind::Bool(value) => (CheckedExprKind::Bool(*value), Type::Bool),
            ast::ExprKind::Str(value) => (CheckedExprKind::Str(value.clone()), Type::String),
            ast::ExprKind::Name(ident) => return self.check_name(ident, expected, ctx),
            ast::ExprKind::Unary { op, operand, .. } => {
                return self.check_unary(*op, operand, span, ctx);
            }
            ast::ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => return self.check_binary(*op, *op_span, lhs, rhs, span, ctx),
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
                if ctx.lookup(&ident.name).is_some() {
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
        if ctx.lookup(&ident.name).is_some() {
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
        if let Some(id) = ctx.lookup(&ident.name) {
            return CheckedExpr {
                kind: CheckedExprKind::Local(id),
                ty: ctx.locals[id.0].ty,
                span: ident.span,
            };
        }
        let name = ident.name.as_str();
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
                Def::List | Def::Map | Def::Box => {
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
        } else if self.by_name.contains_key(name)
            || brim_stdlib::lookup(name).is_some()
            || super::strings::intrinsic_function(name).is_some()
        {
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a function, not a value"))
                    .with_code("E0214")
                    .with_primary(ident.span, "functions cannot be used as values")
                    .with_help(format!("to call it, write `{name}(...)`")),
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
            ast::ExprKind::Name(name) => ctx.lookup(&name.name).is_none(),
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
            Some(def @ (Def::List | Def::Map | Def::Box)) => {
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

    fn int_out_of_range(&mut self, value: u64, span: Span) {
        self.diags.push(
            Diagnostic::error(format!(
                "integer literal `{value}` is out of range for `i64`"
            ))
            .with_code("E0215")
            .with_primary(span, "")
            .with_note(format!(
                "`i64` values range from {} to {}",
                i64::MIN,
                i64::MAX
            )),
        );
    }

    fn check_unary(
        &mut self,
        op: ast::UnaryOp,
        operand: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        // `-9223372036854775808` is i64::MIN, even though the literal alone
        // does not fit in an i64.
        if op == ast::UnaryOp::Neg
            && let ast::ExprKind::Int(value) = operand.kind
            && value == i64::MIN.unsigned_abs()
        {
            return CheckedExpr {
                kind: CheckedExprKind::Int(i64::MIN),
                ty: Type::I64,
                span,
            };
        }
        let operand = self.check_expr(operand, ctx);
        let (op, ty) = match (op, operand.ty) {
            (_, Type::Error) => return error_expr(span),
            (ast::UnaryOp::Neg, Type::I64) => (UnaryOp::IntNeg, Type::I64),
            (ast::UnaryOp::Neg, Type::F64) => (UnaryOp::FloatNeg, Type::F64),
            (ast::UnaryOp::Not, Type::Bool) => (UnaryOp::BoolNot, Type::Bool),
            (op, ty) => {
                let ty = self.show(ty);
                let expected = if op == ast::UnaryOp::Neg {
                    "i64` or `f64"
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
                    .with_note(format!("unary `{}` requires `{expected}`", op.as_str())),
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

    fn check_binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let lhs = self.check_expr(lhs, ctx);
        // `o == None`: the left side tells what the right side is.
        let expected = matches!(op, ast::BinaryOp::Eq | ast::BinaryOp::Ne).then_some(lhs.ty);
        let rhs = self.check_expr_with(rhs, expected, ctx);
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
        use Type::{Bool, F64, I64, String};
        use ast::BinaryOp as Op;

        if lhs.ty == Type::Error || rhs.ty == Type::Error {
            return error_expr(span);
        }
        let resolved = match (op, lhs.ty, rhs.ty) {
            (Op::Add, I64, I64) => Some((BinaryOp::IntAdd, I64)),
            (Op::Sub, I64, I64) => Some((BinaryOp::IntSub, I64)),
            (Op::Mul, I64, I64) => Some((BinaryOp::IntMul, I64)),
            (Op::Div, I64, I64) => Some((BinaryOp::IntDiv, I64)),
            (Op::Rem, I64, I64) => Some((BinaryOp::IntRem, I64)),
            (Op::Add, String, String) => Some((BinaryOp::StrConcat, String)),
            (Op::Eq, I64, I64) => Some((BinaryOp::IntEq, Bool)),
            (Op::Ne, I64, I64) => Some((BinaryOp::IntNe, Bool)),
            (Op::Eq, Bool, Bool) => Some((BinaryOp::BoolEq, Bool)),
            (Op::Ne, Bool, Bool) => Some((BinaryOp::BoolNe, Bool)),
            (Op::Eq, String, String) => Some((BinaryOp::StrEq, Bool)),
            (Op::Ne, String, String) => Some((BinaryOp::StrNe, Bool)),
            (Op::Lt, I64, I64) => Some((BinaryOp::IntLt, Bool)),
            (Op::Le, I64, I64) => Some((BinaryOp::IntLe, Bool)),
            (Op::Gt, I64, I64) => Some((BinaryOp::IntGt, Bool)),
            (Op::Ge, I64, I64) => Some((BinaryOp::IntGe, Bool)),
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
                if a == b && matches!(a, Type::Named(_)) && self.not_comparable(a).is_none() =>
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
            self.not_comparable(lhs.ty)
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
            (Op::Eq | Op::Ne, Some(inner)) => {
                let inner = self.show(inner);
                diag.with_note(format!(
                    "`{l}` holds a `{inner}`, which cannot be compared with `{sym}`"
                ))
            }
            (op, _) => match op {
            Op::Eq | Op::Ne if param => diag.with_note(format!(
                "`{l}` can be any type, and not every type can be compared with `{sym}`"
            )),
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
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => {
                diag.with_note(format!("`{sym}` requires two `i64` or two `f64` operands"))
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => diag.with_note(format!(
                "`{sym}` compares two numbers of the same type or two `string` values"
            )),
            Op::Eq | Op::Ne => diag.with_note(format!("`{sym}` compares two values of the same type")),
            Op::And | Op::Or => diag.with_note(format!("`{sym}` requires two `bool` operands")),
            },
        };
        self.diags.push(diag);
    }

    /// `print(value)` or `to_string(value)` for a struct, enum, list or map.
    fn show_value(&mut self, name: &str, mut args: Vec<CheckedExpr>, span: Span) -> CheckedExpr {
        let value = args.pop().expect("one argument");
        if let Some(inner) = self.not_comparable(value.ty) {
            let ty = self.show(value.ty);
            let why = if matches!(inner, Type::Param(_)) {
                "which can be any type, and not every type has a text form"
            } else {
                "which has no text form"
            };
            let inner = self.show(inner);
            self.diags.push(
                Diagnostic::error(format!("`{name}` cannot show a `{ty}`"))
                    .with_code("E0206")
                    .with_primary(value.span, "unsupported argument type")
                    .with_note(format!("`{ty}` holds a `{inner}`, {why}")),
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

        if let Some(local) = ctx.lookup(name) {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a variable, not a function"))
                    .with_code("E0214")
                    .with_primary(callee.span, "cannot be called")
                    .with_secondary(
                        ctx.locals[local.0].span,
                        format!("`{name}` is declared here"),
                    ),
            );
            return error_expr(span);
        }

        if let Some(&id) = self.by_name.get(name) {
            return self.finish_call(id, None, &arg_refs, span, expected, ctx);
        }
        if self.type_by_name.get(name) == Some(&Def::Box) {
            return self.check_box_new(callee, args, span, expected, ctx);
        }
        if let Some(result) = self.check_intrinsic_call(callee, args, span, ctx) {
            return result;
        }

        // `__` builtins are the standard library modules' own primitives.
        let builtin = brim_stdlib::lookup(name)
            .filter(|b| !brim_stdlib::is_internal(b.name) || self.in_std_module());
        if let Some(builtin) = builtin {
            let args: Vec<CheckedExpr> = args.iter().map(|a| self.check_expr(a, ctx)).collect();
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
                && matches!(arg.ty, Type::Named(_))
            {
                return self.show_value(name, args, span);
            }
            let prims = args
                .iter()
                .map(|a| a.ty.to_prim())
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
            Some(Def::List | Def::Map | Def::Box) => {
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
