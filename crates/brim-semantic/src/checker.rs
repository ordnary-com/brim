mod collections;
mod decls;
mod exprs;
mod matching;

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span, has_errors};

use crate::checked::{
    CheckedEnum, CheckedExpr, CheckedExprKind, CheckedField, CheckedFn, CheckedProgram,
    CheckedStmt, CheckedStruct, CheckedVariant, FnId, Local, LocalId,
};
use crate::{Def, EnumId, StructId, Type, TypeTable};

/// A resolved function signature.
struct Signature {
    name_span: Span,
    type_params: Vec<String>,
    params: Vec<ParamSig>,
    ret: Type,
    /// Span of the `-> type` annotation, if written.
    ret_span: Option<Span>,
}

struct ParamSig {
    name: String,
    ty: Type,
    span: Span,
}

/// A declared struct.
struct StructInfo {
    name: String,
    name_span: Span,
    span: Span,
    type_params: Vec<String>,
    fields: Vec<FieldInfo>,
}

struct FieldInfo {
    name: String,
    ty: Type,
    span: Span,
}

/// A declared or built-in enum.
struct EnumInfo {
    name: String,
    /// `None` for the built-in enums, which have no source location.
    name_span: Option<Span>,
    type_params: Vec<String>,
    variants: Vec<VariantInfo>,
}

struct VariantInfo {
    name: String,
    fields: Vec<Type>,
    span: Option<Span>,
}

pub(crate) struct Checker<'a> {
    diags: Vec<Diagnostic>,
    types: TypeTable,
    /// One entry per AST struct, in declaration order.
    structs: Vec<StructInfo>,
    /// The built-in enums, then one entry per AST enum.
    enums: Vec<EnumInfo>,
    /// Struct and enum names to their declarations.
    type_by_name: HashMap<String, Def>,
    /// Variants usable without their enum's name: `Some`, `None`, `Ok`, `Err`.
    prelude_variants: HashMap<&'static str, (EnumId, usize)>,
    /// One signature per AST function, in declaration order.
    sigs: Vec<Signature>,
    /// Function names to the first function declared with that name.
    by_name: HashMap<&'a str, FnId>,
    /// Type parameter names of the declaration being checked, for resolving
    /// and showing `Type::Param`.
    type_params: Vec<String>,
    /// `["T"]`, the type parameters of `List`.
    list_type_params: Vec<String>,
}

/// Per-function checking state.
struct FnCtx {
    locals: Vec<Local>,
    /// Whether each local is ever assigned after its declaration.
    reassigned: Vec<bool>,
    scopes: Vec<HashMap<String, LocalId>>,
    /// One entry per enclosing loop: whether it contains a `break`.
    loops: Vec<bool>,
    /// The first `param_count` locals are the parameters.
    param_count: usize,
    ret: Type,
    ret_span: Option<Span>,
}

impl FnCtx {
    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn visible_names(&self) -> impl Iterator<Item = &str> {
        self.scopes
            .iter()
            .flat_map(|scope| scope.keys().map(String::as_str))
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// `"1 was"` or `"3 were"`.
fn supplied(n: usize) -> String {
    if n == 1 {
        "1 was".to_string()
    } else {
        format!("{n} were")
    }
}

fn error_expr(span: Span) -> CheckedExpr {
    CheckedExpr {
        kind: CheckedExprKind::Bool(false),
        ty: Type::Error,
        span,
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn check(program: &'a ast::Program) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
        let mut checker = Checker {
            diags: Vec::new(),
            types: TypeTable::default(),
            structs: Vec::new(),
            enums: Vec::new(),
            type_by_name: HashMap::new(),
            prelude_variants: HashMap::new(),
            sigs: Vec::new(),
            by_name: HashMap::new(),
            type_params: Vec::new(),
            list_type_params: vec!["T".into()],
        };
        checker.add_prelude();
        checker.collect_types(program);
        checker.collect_functions(program);
        let entry = checker.check_entry();
        let functions: Vec<CheckedFn> = program
            .functions
            .iter()
            .zip(0..)
            .map(|(f, i)| checker.check_fn(f, FnId(i)))
            .collect();

        let mut diags = checker.diags;
        // Source order; diagnostics without a location go last.
        diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
        if has_errors(&diags) {
            return (None, diags);
        }
        let entry = entry.expect("a missing entry point is reported as an error");
        let structs = checker
            .structs
            .into_iter()
            .map(|s| CheckedStruct {
                name: s.name,
                span: s.span,
                type_params: s.type_params,
                fields: s
                    .fields
                    .into_iter()
                    .map(|f| CheckedField {
                        name: f.name,
                        ty: f.ty,
                    })
                    .collect(),
            })
            .collect();
        let enums = checker
            .enums
            .into_iter()
            .map(|e| CheckedEnum {
                name: e.name,
                type_params: e.type_params,
                variants: e
                    .variants
                    .into_iter()
                    .map(|v| CheckedVariant {
                        name: v.name,
                        fields: v.fields,
                    })
                    .collect(),
            })
            .collect();
        (
            Some(CheckedProgram {
                types: checker.types,
                structs,
                enums,
                functions,
                entry,
            }),
            diags,
        )
    }

    /// A type as users write it, for messages.
    fn show(&self, ty: Type) -> String {
        match ty {
            Type::Param(i) => self
                .type_params
                .get(i as usize)
                .cloned()
                .unwrap_or_else(|| format!("T{i}")),
            Type::Named(id) => {
                let named = self.types.get(id);
                let name = self.def_name(named.def);
                if named.args.is_empty() {
                    name.to_string()
                } else {
                    let args: Vec<String> = named.args.iter().map(|t| self.show(*t)).collect();
                    format!("{name}[{}]", args.join(", "))
                }
            }
            other => other.to_string(),
        }
    }

    fn def_name(&self, def: Def) -> &str {
        match def {
            Def::Struct(id) => &self.structs[id.0].name,
            Def::Enum(id) => &self.enums[id.0].name,
            Def::List => "List",
        }
    }

    /// The struct a type refers to, with its type arguments.
    fn as_struct(&self, ty: Type) -> Option<(StructId, Vec<Type>)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        match named.def {
            Def::Struct(s) => Some((s, named.args.clone())),
            Def::Enum(_) | Def::List => None,
        }
    }

    /// The enum a type refers to, with its type arguments.
    fn as_enum(&self, ty: Type) -> Option<(EnumId, Vec<Type>)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        match named.def {
            Def::Enum(e) => Some((e, named.args.clone())),
            Def::Struct(_) | Def::List => None,
        }
    }

    // ---- function bodies ----------------------------------------------

    fn check_fn(&mut self, f: &ast::FnDecl, id: FnId) -> CheckedFn {
        let sig = &self.sigs[id.0];
        self.type_params = sig.type_params.clone();
        let mut ctx = FnCtx {
            locals: Vec::new(),
            reassigned: Vec::new(),
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            param_count: sig.params.len(),
            ret: sig.ret,
            ret_span: sig.ret_span,
        };
        for param in &sig.params {
            let local = LocalId(ctx.locals.len());
            ctx.locals.push(Local {
                name: param.name.clone(),
                ty: param.ty,
                mutable: false,
                span: param.span,
            });
            ctx.reassigned.push(false);
            // For duplicate parameters (already reported), the first one wins.
            ctx.scopes[0].entry(param.name.clone()).or_insert(local);
        }
        let param_count = ctx.locals.len();

        // The body shares its scope with the parameters, so `let a` in the
        // body conflicts with a parameter `a`.
        let (body, diverges) = self.check_stmts(&f.body.stmts, &mut ctx);

        for (local, reassigned) in ctx.locals.iter().zip(&ctx.reassigned) {
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

        if !diverges && !matches!(ctx.ret, Type::Void | Type::Error) {
            let close = Span::new(f.body.span.end.saturating_sub(1), f.body.span.end);
            let mut diag = Diagnostic::error(format!(
                "function `{}` may end without returning a value",
                f.name.name
            ))
            .with_code("E0207")
            .with_primary(close, "missing `return` before the end of the function");
            if let Some(span) = ctx.ret_span {
                let ret = self.show(ctx.ret);
                diag = diag.with_secondary(span, format!("`{ret}` must be returned"));
            }
            self.diags.push(diag);
        }

        let type_params = std::mem::take(&mut self.type_params);
        CheckedFn {
            name: f.name.name.clone(),
            span: f.span,
            type_params,
            param_count,
            ret: ctx.ret,
            locals: ctx.locals,
            body,
        }
    }

    /// Checks a statement list. Returns the checked statements and whether
    /// the list *diverges*: control never continues past its end, because
    /// every path returns, breaks, continues or loops forever.
    fn check_stmts(&mut self, stmts: &[ast::Stmt], ctx: &mut FnCtx) -> (Vec<CheckedStmt>, bool) {
        let mut out = Vec::with_capacity(stmts.len());
        let mut returned_at: Option<Span> = None;
        let mut warned = false;
        for stmt in stmts {
            if let Some(span) = returned_at
                && !warned
            {
                warned = true;
                self.diags.push(
                    Diagnostic::warning("unreachable code")
                        .with_code("W0001")
                        .with_primary(stmt.span, "this statement will never run")
                        .with_secondary(span, "control never continues past this"),
                );
            }
            let (checked, returns) = self.check_stmt(stmt, ctx);
            out.push(checked);
            if returns && returned_at.is_none() {
                returned_at = Some(stmt.span);
            }
        }
        (out, returned_at.is_some())
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt, ctx: &mut FnCtx) -> (CheckedStmt, bool) {
        match &stmt.kind {
            ast::StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let declared = self.resolve_type(ty, "a variable");
                let expected = (declared != Type::Error).then_some(declared);
                let init = self.check_expr_with(init, expected, ctx);
                self.expect_type(
                    &init,
                    declared,
                    Some(ty.span()),
                    "expected due to this type",
                );
                let local = self.declare(name, declared, *mutable, ctx);
                (CheckedStmt::Let { local, init }, false)
            }
            ast::StmtKind::Assign {
                target,
                path,
                op,
                op_span,
                value,
            } => (
                self.check_assign(target, path, *op, *op_span, value, ctx),
                false,
            ),
            ast::StmtKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                let cond = self.check_condition(cond, "if", ctx);
                let (then_body, then_diverges) = self.check_block(&then_block.stmts, ctx);
                let (else_body, else_diverges) = match else_branch.as_deref() {
                    None => (Vec::new(), false),
                    Some(ast::Stmt {
                        kind: ast::StmtKind::Block(block),
                        ..
                    }) => self.check_block(&block.stmts, ctx),
                    Some(other) => {
                        let (stmt, diverges) = self.check_stmt(other, ctx);
                        (vec![stmt], diverges)
                    }
                };
                let stmt = CheckedStmt::If {
                    cond,
                    then_body,
                    else_body,
                };
                (stmt, then_diverges && else_diverges)
            }
            ast::StmtKind::While { cond, body } => {
                let cond = self.check_condition(cond, "while", ctx);
                ctx.loops.push(false);
                let (body, _) = self.check_block(&body.stmts, ctx);
                let has_break = ctx.loops.pop().expect("pushed above");
                // `while true` without `break` never finishes.
                let forever = matches!(cond.kind, CheckedExprKind::Bool(true)) && !has_break;
                (CheckedStmt::While { cond, body }, forever)
            }
            ast::StmtKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, ctx),
            ast::StmtKind::For { var, iter, body } => self.check_for(var, iter, body, ctx),
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                let is_break = matches!(stmt.kind, ast::StmtKind::Break);
                let keyword = if is_break { "break" } else { "continue" };
                match ctx.loops.last_mut() {
                    Some(has_break) => *has_break |= is_break,
                    None => self.diags.push(
                        Diagnostic::error(format!("`{keyword}` outside of a loop"))
                            .with_code("E0217")
                            .with_primary(stmt.span, format!("cannot `{keyword}` here"))
                            .with_note(format!(
                                "`{keyword}` can only be used inside a `while` loop"
                            )),
                    ),
                }
                let checked = if is_break {
                    CheckedStmt::Break
                } else {
                    CheckedStmt::Continue
                };
                (checked, true)
            }
            ast::StmtKind::Return(value) => {
                let expected = (!matches!(ctx.ret, Type::Void | Type::Error)).then_some(ctx.ret);
                let value = value
                    .as_ref()
                    .map(|v| self.check_expr_with(v, expected, ctx));
                self.check_return(value.as_ref(), stmt.span, ctx);
                (
                    CheckedStmt::Return {
                        value,
                        span: stmt.span,
                    },
                    true,
                )
            }
            ast::StmtKind::Expr(expr) => {
                let checked = self.check_expr(expr, ctx);
                let is_call = |e: &CheckedExpr| {
                    matches!(
                        e.kind,
                        CheckedExprKind::Call { .. }
                            | CheckedExprKind::Builtin { .. }
                            | CheckedExprKind::ListOp { .. }
                    )
                };
                // `do_something()?` is a call too: only its error is used.
                let is_call = is_call(&checked)
                    || matches!(&checked.kind, CheckedExprKind::Try { operand } if is_call(operand));
                if !is_call && checked.ty != Type::Error {
                    let ty = self.show(checked.ty);
                    self.diags.push(
                        Diagnostic::warning("unused value")
                            .with_code("W0002")
                            .with_primary(checked.span, format!("this `{ty}` value is never used"))
                            .with_help("store it with `let`, or remove the expression"),
                    );
                }
                (CheckedStmt::Expr(checked), false)
            }
            ast::StmtKind::Block(block) => {
                let (stmts, diverges) = self.check_block(&block.stmts, ctx);
                (CheckedStmt::Block(stmts), diverges)
            }
        }
    }

    /// Checks statements in a new scope.
    fn check_block(&mut self, stmts: &[ast::Stmt], ctx: &mut FnCtx) -> (Vec<CheckedStmt>, bool) {
        ctx.scopes.push(HashMap::new());
        let result = self.check_stmts(stmts, ctx);
        ctx.scopes.pop();
        result
    }

    /// Checks the condition of an `if` or `while`; it must be a `bool`.
    fn check_condition(&mut self, cond: &ast::Expr, keyword: &str, ctx: &mut FnCtx) -> CheckedExpr {
        let cond = self.check_expr(cond, ctx);
        if !matches!(cond.ty, Type::Bool | Type::Error) {
            let found = self.show(cond.ty);
            let mut diag = Diagnostic::error("mismatched types")
                .with_code("E0206")
                .with_primary(cond.span, format!("expected `bool`, found `{found}`"))
                .with_note(format!("the condition of `{keyword}` must be a `bool`"));
            if cond.ty == Type::I64 {
                diag = diag.with_help("compare explicitly, for example `x != 0`");
            }
            self.diags.push(diag);
        }
        cond
    }

    fn check_assign(
        &mut self,
        target: &ast::Ident,
        path: &[ast::PlaceElem],
        op: Option<ast::BinaryOp>,
        op_span: Span,
        value: &ast::Expr,
        ctx: &mut FnCtx,
    ) -> CheckedStmt {
        let steps: Vec<collections::StepRef<'_>> = path
            .iter()
            .map(|elem| match elem {
                ast::PlaceElem::Field(field) => collections::StepRef::Field(field),
                ast::PlaceElem::Index(index) => collections::StepRef::Index(index),
            })
            .collect();
        let Some(place) = self.resolve_place(target, &steps, ctx) else {
            let value = self.check_expr(value, ctx);
            return CheckedStmt::Expr(value);
        };
        let local = &ctx.locals[place.local.0];
        let (var_ty, mutable, decl_span) = (local.ty, local.mutable, local.span);
        let is_param = place.local.0 < ctx.param_count;
        let ty = place.ty;
        let expected = (op.is_none() && ty != Type::Error).then_some(ty);
        let value = self.check_expr_with(value, expected, ctx);

        if !mutable {
            let ty = self.show(var_ty);
            let message = if path.is_empty() {
                format!("cannot assign to immutable variable `{}`", target.name)
            } else {
                format!(
                    "cannot assign to a part of immutable variable `{}`",
                    target.name
                )
            };
            let mut diag = Diagnostic::error(message)
                .with_code("E0218")
                .with_primary(
                    target.span,
                    if path.is_empty() {
                        "cannot be reassigned"
                    } else {
                        "it cannot be changed"
                    },
                )
                .with_secondary(decl_span, "declared here");
            diag = if is_param {
                diag.with_note("function parameters cannot be reassigned")
                    .with_help(format!(
                        "copy it into a mutable variable: `let mut {}: {ty} = {}`",
                        target.name, target.name
                    ))
            } else {
                diag.with_help(format!("make it mutable: `let mut {}`", target.name))
            };
            self.diags.push(diag);
        } else {
            ctx.reassigned[place.local.0] = true;
        }

        let reason = if path.is_empty() {
            "variable declared here"
        } else {
            "the variable's type is declared here"
        };
        let (op, value) = match op {
            None => {
                self.expect_type(&value, ty, Some(decl_span), reason);
                (None, value)
            }
            Some(op) => {
                // Type-check `place op value` with a stand-in for the place.
                let current = CheckedExpr {
                    kind: CheckedExprKind::Bool(false),
                    ty,
                    span: target.span,
                };
                let span = target.span.to(value.span);
                let value_span = value.span;
                let combined = self.resolve_binary(op, op_span, current, value, span);
                self.expect_type(&combined, ty, Some(decl_span), reason);
                match combined.kind {
                    CheckedExprKind::Binary { op, rhs, .. } => (Some(op), *rhs),
                    // An error was reported.
                    _ => (None, error_expr(value_span)),
                }
            }
        };
        CheckedStmt::Assign { place, op, value }
    }

    fn declare(&mut self, name: &ast::Ident, ty: Type, mutable: bool, ctx: &mut FnCtx) -> LocalId {
        let id = LocalId(ctx.locals.len());
        if let Some(previous) = ctx.scopes.last().and_then(|s| s.get(&name.name)) {
            let previous_span = ctx.locals[previous.0].span;
            self.diags.push(
                Diagnostic::error(format!(
                    "variable `{}` is already declared in this scope",
                    name.name
                ))
                .with_code("E0203")
                .with_primary(name.span, "declared again here")
                .with_secondary(previous_span, "first declared here")
                .with_help("use a different name, or declare it inside a nested block"),
            );
        }
        ctx.locals.push(Local {
            name: name.name.clone(),
            ty,
            mutable,
            span: name.span,
        });
        ctx.reassigned.push(false);
        ctx.scopes
            .last_mut()
            .expect("there is always a scope")
            .insert(name.name.clone(), id);
        id
    }

    fn check_return(&mut self, value: Option<&CheckedExpr>, span: Span, ctx: &FnCtx) {
        match (value, ctx.ret) {
            (_, Type::Error) | (None, Type::Void) => {}
            (None, ret) => {
                let ret = self.show(ret);
                let mut diag = Diagnostic::error("missing return value")
                    .with_code("E0208")
                    .with_primary(span, format!("expected a `{ret}` value"));
                if let Some(ret_span) = ctx.ret_span {
                    diag = diag.with_secondary(ret_span, "return type declared here");
                }
                self.diags.push(diag);
            }
            (Some(value), Type::Void) => {
                let mut diag = Diagnostic::error(
                    "cannot return a value from a function without a return type",
                )
                .with_code("E0208")
                .with_primary(value.span, "unexpected return value");
                if !matches!(value.ty, Type::Void | Type::Error) {
                    diag = diag.with_help(format!(
                        "declare the return type with `-> {}` after the parameter list",
                        self.show(value.ty)
                    ));
                }
                self.diags.push(diag);
            }
            (Some(value), ret) => {
                self.expect_type(value, ret, ctx.ret_span, "expected due to this return type");
            }
        }
    }

    /// Reports a mismatch if `expr` does not have type `expected`.
    fn expect_type(
        &mut self,
        expr: &CheckedExpr,
        expected: Type,
        reason: Option<Span>,
        reason_label: &str,
    ) {
        if expr.ty == expected || expr.ty == Type::Error || expected == Type::Error {
            return;
        }
        let mut diag = Diagnostic::error("mismatched types")
            .with_code("E0206")
            .with_primary(
                expr.span,
                format!(
                    "expected `{}`, found `{}`",
                    self.show(expected),
                    self.show(expr.ty)
                ),
            );
        if let Some(span) = reason {
            diag = diag.with_secondary(span, reason_label);
        }
        if expr.ty == Type::Void {
            diag = diag.with_note("this expression calls a function that does not return a value");
        } else {
            diag = diag.with_note("Brim never converts between types implicitly");
        }
        self.diags.push(diag);
    }
}
