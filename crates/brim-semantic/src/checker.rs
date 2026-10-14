use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span, has_errors};

use crate::Type;
use crate::checked::{
    BinaryOp, CheckedExpr, CheckedExprKind, CheckedFn, CheckedProgram, CheckedStmt, FnId, Local,
    LocalId, UnaryOp,
};
use crate::suggest;

/// A resolved function signature.
struct Signature {
    name_span: Span,
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

pub(crate) struct Checker<'a> {
    diags: Vec<Diagnostic>,
    /// One signature per AST function, in declaration order.
    sigs: Vec<Signature>,
    /// Function names to the first function declared with that name.
    by_name: HashMap<&'a str, FnId>,
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
            sigs: Vec::new(),
            by_name: HashMap::new(),
        };
        checker.collect(program);
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
        (Some(CheckedProgram { functions, entry }), diags)
    }

    // ---- declarations -------------------------------------------------

    fn collect(&mut self, program: &'a ast::Program) {
        for (i, f) in program.functions.iter().enumerate() {
            let name = f.name.name.as_str();
            if brim_stdlib::lookup(name).is_some() {
                self.diags.push(
                    Diagnostic::error(format!("cannot redefine builtin function `{name}`"))
                        .with_code("E0216")
                        .with_primary(f.name.span, "this name belongs to a builtin")
                        .with_help("choose a different name"),
                );
            } else if let Some(first) = self.by_name.get(name) {
                let first_span = self.sigs[first.0].name_span;
                self.diags.push(
                    Diagnostic::error(format!("function `{name}` is defined more than once"))
                        .with_code("E0202")
                        .with_primary(f.name.span, "redefined here")
                        .with_secondary(first_span, "first defined here"),
                );
            } else {
                self.by_name.insert(name, FnId(i));
            }

            let mut params: Vec<ParamSig> = Vec::new();
            for param in &f.params {
                if let Some(previous) = params.iter().find(|p| p.name == param.name.name) {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "parameter `{}` is declared more than once",
                            param.name.name
                        ))
                        .with_code("E0204")
                        .with_primary(param.name.span, "duplicate parameter")
                        .with_secondary(previous.span, "first declared here"),
                    );
                }
                let ty = self.resolve_type(&param.ty, "a parameter");
                params.push(ParamSig {
                    name: param.name.name.clone(),
                    ty,
                    span: param.span,
                });
            }
            let ret = match &f.ret {
                Some(ty) => self.resolve_type(ty, "return"),
                None => Type::Void,
            };
            self.sigs.push(Signature {
                name_span: f.name.span,
                params,
                ret,
                ret_span: f.ret.as_ref().map(ast::TypeExpr::span),
            });
        }
    }

    /// Resolves a written type. `what` describes where it appears; `void` is
    /// only accepted for return types.
    fn resolve_type(&mut self, ty: &ast::TypeExpr, what: &str) -> Type {
        let name = ty.name.name.as_str();
        match Type::from_name(name) {
            Some(Type::Void) if what != "return" => {
                self.diags.push(
                    Diagnostic::error(format!("`void` cannot be the type of {what}"))
                        .with_code("E0213")
                        .with_primary(ty.span(), "`void` has no values")
                        .with_note("`void` is only valid as a function return type"),
                );
                Type::Error
            }
            Some(t) => t,
            None => {
                let mut diag = Diagnostic::error(format!("unknown type `{name}`"))
                    .with_code("E0212")
                    .with_primary(ty.span(), "not a known type");
                diag = match suggest::type_name(name) {
                    Some(s) => diag.with_help(format!("did you mean `{s}`?")),
                    None => {
                        diag.with_note("the available types are `i64`, `bool`, `string` and `void`")
                    }
                };
                self.diags.push(diag);
                Type::Error
            }
        }
    }

    fn check_entry(&mut self) -> Option<FnId> {
        let Some(&id) = self.by_name.get("main") else {
            self.diags.push(
                Diagnostic::error("no `main` function found")
                    .with_code("E0210")
                    .with_note("every Brim program starts running at its `main` function")
                    .with_help("add an entry point: `fn main() { ... }`"),
            );
            return None;
        };
        let sig = &self.sigs[id.0];
        if let (Some(first), Some(last)) = (sig.params.first(), sig.params.last()) {
            let span = first.span.to(last.span);
            self.diags.push(
                Diagnostic::error("`main` cannot take parameters")
                    .with_code("E0211")
                    .with_primary(span, "remove these parameters")
                    .with_note("command-line arguments are not supported yet"),
            );
        }
        if !matches!(sig.ret, Type::Void | Type::Error)
            && let Some(span) = sig.ret_span
        {
            self.diags.push(
                Diagnostic::error("`main` cannot return a value")
                    .with_code("E0211")
                    .with_primary(span, "remove this return type")
                    .with_note("exit codes are not supported yet"),
            );
        }
        Some(id)
    }

    // ---- function bodies ----------------------------------------------

    fn check_fn(&mut self, f: &ast::FnDecl, id: FnId) -> CheckedFn {
        let sig = &self.sigs[id.0];
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
                diag = diag.with_secondary(span, format!("`{}` must be returned", ctx.ret));
            }
            self.diags.push(diag);
        }

        CheckedFn {
            name: f.name.name.clone(),
            span: f.span,
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
                let init = self.check_expr(init, ctx);
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
                op,
                op_span,
                value,
            } => (self.check_assign(target, *op, *op_span, value, ctx), false),
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
                let value = value.as_ref().map(|v| self.check_expr(v, ctx));
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
                let is_call = matches!(
                    checked.kind,
                    CheckedExprKind::Call { .. } | CheckedExprKind::Builtin { .. }
                );
                if !is_call && checked.ty != Type::Error {
                    self.diags.push(
                        Diagnostic::warning("unused value")
                            .with_code("W0002")
                            .with_primary(
                                checked.span,
                                format!("this `{}` value is never used", checked.ty),
                            )
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
            let mut diag = Diagnostic::error("mismatched types")
                .with_code("E0206")
                .with_primary(cond.span, format!("expected `bool`, found `{}`", cond.ty))
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
        op: Option<ast::BinaryOp>,
        op_span: Span,
        value: &ast::Expr,
        ctx: &mut FnCtx,
    ) -> CheckedStmt {
        let value = self.check_expr(value, ctx);
        let Some(id) = ctx.lookup(&target.name) else {
            // Reports the undefined name (or a function used as a value).
            self.check_name(target, ctx);
            return CheckedStmt::Expr(value);
        };
        let local = &ctx.locals[id.0];
        let (ty, mutable, decl_span) = (local.ty, local.mutable, local.span);
        let is_param = id.0 < ctx.param_count;
        if !mutable {
            let mut diag = Diagnostic::error(format!(
                "cannot assign to immutable variable `{}`",
                target.name
            ))
            .with_code("E0218")
            .with_primary(target.span, "cannot be reassigned")
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
            ctx.reassigned[id.0] = true;
        }

        let value = match op {
            None => value,
            Some(op) => {
                let current = CheckedExpr {
                    kind: CheckedExprKind::Local(id),
                    ty,
                    span: target.span,
                };
                let span = target.span.to(value.span);
                self.resolve_binary(op, op_span, current, value, span)
            }
        };
        self.expect_type(&value, ty, Some(decl_span), "variable declared here");
        CheckedStmt::Assign { local: id, value }
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
                        value.ty
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
                format!("expected `{expected}`, found `{}`", expr.ty),
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

    // ---- expressions --------------------------------------------------

    fn check_expr(&mut self, expr: &ast::Expr, ctx: &mut FnCtx) -> CheckedExpr {
        let span = expr.span;
        let (kind, ty) = match &expr.kind {
            ast::ExprKind::Int(value) => match i64::try_from(*value) {
                Ok(v) => (CheckedExprKind::Int(v), Type::I64),
                Err(_) => {
                    self.int_out_of_range(*value, span);
                    return error_expr(span);
                }
            },
            ast::ExprKind::Bool(value) => (CheckedExprKind::Bool(*value), Type::Bool),
            ast::ExprKind::Str(value) => (CheckedExprKind::Str(value.clone()), Type::String),
            ast::ExprKind::Name(ident) => return self.check_name(ident, ctx),
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
                return self.check_call(callee, args, span, ctx);
            }
        };
        CheckedExpr { kind, ty, span }
    }

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

    fn check_name(&mut self, ident: &ast::Ident, ctx: &FnCtx) -> CheckedExpr {
        if let Some(id) = ctx.lookup(&ident.name) {
            return CheckedExpr {
                kind: CheckedExprKind::Local(id),
                ty: ctx.locals[id.0].ty,
                span: ident.span,
            };
        }
        let name = ident.name.as_str();
        if self.by_name.contains_key(name) || brim_stdlib::lookup(name).is_some() {
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
            (ast::UnaryOp::Not, Type::Bool) => (UnaryOp::BoolNot, Type::Bool),
            (op, ty) => {
                let expected = if op == ast::UnaryOp::Neg {
                    "i64"
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
        let rhs = self.check_expr(rhs, ctx);
        self.resolve_binary(op, op_span, lhs, rhs, span)
    }

    /// Selects the type-specific operation for `lhs op rhs`.
    fn resolve_binary(
        &mut self,
        op: ast::BinaryOp,
        op_span: Span,
        lhs: CheckedExpr,
        rhs: CheckedExpr,
        span: Span,
    ) -> CheckedExpr {
        use Type::{Bool, I64, String};
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
            (Op::And | Op::Or, Bool, Bool) => None,
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
        let (l, r) = (lhs.ty, rhs.ty);
        let mut diag = Diagnostic::error(format!("cannot apply `{sym}` to `{l}` and `{r}`"))
            .with_code("E0209")
            .with_primary(op_span, format!("`{l} {sym} {r}` is not defined"))
            .with_secondary(lhs.span, format!("`{l}`"))
            .with_secondary(rhs.span, format!("`{r}`"));
        diag = match op {
            Op::Add if l == Type::String || r == Type::String => diag
                .with_note("`+` joins two strings or adds two integers")
                .with_note("Brim never converts values to strings implicitly"),
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Rem => {
                diag.with_note(format!("`{sym}` requires two `i64` operands"))
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                diag.with_note(format!("`{sym}` compares two `i64` values"))
            }
            Op::Eq | Op::Ne => diag.with_note(format!(
                "`{sym}` compares two values of the same type (`i64`, `bool` or `string`)"
            )),
            Op::And | Op::Or => diag.with_note(format!("`{sym}` requires two `bool` operands")),
        };
        self.diags.push(diag);
    }

    fn check_call(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let args: Vec<CheckedExpr> = args.iter().map(|a| self.check_expr(a, ctx)).collect();
        let name = callee.name.as_str();

        if let Some(local) = ctx.lookup(name) {
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
            let sig = &self.sigs[id.0];
            let ret = sig.ret;
            if args.len() != sig.params.len() {
                let diag = Diagnostic::error(format!(
                    "function `{name}` takes {} but {} supplied",
                    plural(sig.params.len(), "argument"),
                    if args.len() == 1 {
                        "1 was".to_string()
                    } else {
                        format!("{} were", args.len())
                    }
                ))
                .with_code("E0205")
                .with_primary(
                    span,
                    format!("expected {}", plural(sig.params.len(), "argument")),
                )
                .with_secondary(sig.name_span, format!("`{name}` is defined here"));
                self.diags.push(diag);
            } else {
                let expected: Vec<(Type, Span)> =
                    sig.params.iter().map(|p| (p.ty, p.span)).collect();
                for (arg, (ty, param_span)) in args.iter().zip(expected) {
                    self.expect_type(arg, ty, Some(param_span), "parameter declared here");
                }
            }
            return CheckedExpr {
                kind: CheckedExprKind::Call { func: id, args },
                ty: ret,
                span,
            };
        }

        if let Some(builtin) = brim_stdlib::lookup(name) {
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
                        if args.len() == 1 {
                            "1 was".to_string()
                        } else {
                            format!("{} were", args.len())
                        }
                    ))
                    .with_code("E0205")
                    .with_primary(span, format!("expected {expected} {noun}")),
                );
                return error_expr(span);
            }
            let Some(prims) = args
                .iter()
                .map(|a| a.ty.to_prim())
                .collect::<Option<Vec<_>>>()
            else {
                // An argument already has an error.
                return error_expr(span);
            };
            let Some(overload) = builtin.resolve(&prims) else {
                let found = prims
                    .iter()
                    .map(|p| format!("`{p}`"))
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

        let mut diag = Diagnostic::error(format!("undefined function `{name}`"))
            .with_code("E0201")
            .with_primary(callee.span, "not found");
        let candidates = self
            .by_name
            .keys()
            .copied()
            .chain(brim_stdlib::BUILTINS.iter().map(|b| b.name));
        if let Some(similar) = suggest::similar(name, candidates) {
            diag = diag.with_help(format!(
                "a function with a similar name exists: `{similar}`"
            ));
        }
        self.diags.push(diag);
        error_expr(span)
    }
}
