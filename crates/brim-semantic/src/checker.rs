use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span, has_errors};

use crate::checked::{
    BinaryOp, CheckedExpr, CheckedExprKind, CheckedField, CheckedFn, CheckedProgram, CheckedStmt,
    CheckedStruct, FnId, Local, LocalId, UnaryOp,
};
use crate::suggest;
use crate::{StructId, Type};

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

/// A declared struct.
struct StructInfo {
    name: String,
    name_span: Span,
    span: Span,
    fields: Vec<FieldInfo>,
}

struct FieldInfo {
    name: String,
    ty: Type,
    span: Span,
}

pub(crate) struct Checker<'a> {
    diags: Vec<Diagnostic>,
    /// One entry per AST struct, in declaration order.
    structs: Vec<StructInfo>,
    /// Struct names to the first struct declared with that name.
    struct_by_name: HashMap<&'a str, StructId>,
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
            structs: Vec::new(),
            struct_by_name: HashMap::new(),
            sigs: Vec::new(),
            by_name: HashMap::new(),
        };
        checker.collect_structs(program);
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
        let structs = checker
            .structs
            .into_iter()
            .map(|s| CheckedStruct {
                name: s.name,
                span: s.span,
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
        (
            Some(CheckedProgram {
                structs,
                functions,
                entry,
            }),
            diags,
        )
    }

    /// A type as users write it, for messages.
    fn show(&self, ty: Type) -> String {
        match ty {
            Type::Struct(id) => self.structs[id.0].name.clone(),
            other => other.to_string(),
        }
    }

    // ---- structs ------------------------------------------------------

    fn collect_structs(&mut self, program: &'a ast::Program) {
        // Names first, so fields can refer to structs declared later.
        for (i, decl) in program.structs.iter().enumerate() {
            let name = decl.name.name.as_str();
            if Type::from_name(name).is_some() {
                self.diags.push(
                    Diagnostic::error(format!("`{name}` is a built-in type"))
                        .with_code("E0219")
                        .with_primary(decl.name.span, "cannot be used as a struct name")
                        .with_help("choose a different name"),
                );
            } else if let Some(first) = self.struct_by_name.get(name) {
                let first_span = self.structs[first.0].name_span;
                self.diags.push(
                    Diagnostic::error(format!("struct `{name}` is defined more than once"))
                        .with_code("E0219")
                        .with_primary(decl.name.span, "redefined here")
                        .with_secondary(first_span, "first defined here"),
                );
            } else {
                self.struct_by_name.insert(name, StructId(i));
            }
            self.structs.push(StructInfo {
                name: name.to_string(),
                name_span: decl.name.span,
                span: decl.span,
                fields: Vec::new(),
            });
        }
        for (i, decl) in program.structs.iter().enumerate() {
            let mut fields: Vec<FieldInfo> = Vec::new();
            for field in &decl.fields {
                if let Some(previous) = fields.iter().find(|f| f.name == field.name.name) {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "field `{}` is declared more than once",
                            field.name.name
                        ))
                        .with_code("E0220")
                        .with_primary(field.name.span, "duplicate field")
                        .with_secondary(previous.span, "first declared here"),
                    );
                }
                let ty = self.resolve_type(&field.ty, "a field");
                fields.push(FieldInfo {
                    name: field.name.name.clone(),
                    ty,
                    span: field.span,
                });
            }
            self.structs[i].fields = fields;
        }
        self.check_recursive_structs();
    }

    /// A struct that contains itself, directly or through other structs,
    /// would be infinitely large.
    fn check_recursive_structs(&mut self) {
        let mut reported = vec![false; self.structs.len()];
        for start in 0..self.structs.len() {
            if reported[start] {
                continue;
            }
            // Depth-first search for a path of fields leading back to `start`.
            let mut path: Vec<(usize, usize)> = Vec::new(); // (struct, field)
            let mut visited = vec![false; self.structs.len()];
            if !self.find_cycle(start, start, &mut path, &mut visited) {
                continue;
            }
            let chain: Vec<String> = path
                .iter()
                .map(|&(s, f)| {
                    format!(
                        "{}.{}",
                        self.structs[s].name, self.structs[s].fields[f].name
                    )
                })
                .collect();
            for &(s, _) in &path {
                reported[s] = true;
            }
            let (first_struct, first_field) = path[0];
            let field = &self.structs[first_struct].fields[first_field];
            let name = &self.structs[start].name;
            self.diags.push(
                Diagnostic::error(format!("struct `{name}` contains itself"))
                    .with_code("E0221")
                    .with_primary(self.structs[start].name_span, "infinitely large")
                    .with_secondary(field.span, format!("`{name}` contains itself through here"))
                    .with_note(format!("the cycle is: {} -> {name}", chain.join(" -> ")))
                    .with_note("structs are values: every field is stored inside its struct"),
            );
        }
    }

    fn find_cycle(
        &self,
        current: usize,
        target: usize,
        path: &mut Vec<(usize, usize)>,
        visited: &mut [bool],
    ) -> bool {
        if visited[current] {
            return false;
        }
        visited[current] = true;
        for (f, field) in self.structs[current].fields.iter().enumerate() {
            let Type::Struct(next) = field.ty else {
                continue;
            };
            path.push((current, f));
            if next.0 == target || self.find_cycle(next.0, target, path, visited) {
                return true;
            }
            path.pop();
        }
        false
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
            } else if let Some(s) = self.struct_by_name.get(name) {
                self.diags.push(
                    Diagnostic::error(format!("`{name}` is already the name of a struct"))
                        .with_code("E0219")
                        .with_primary(f.name.span, "function declared here")
                        .with_secondary(self.structs[s.0].name_span, "struct declared here")
                        .with_help("functions and structs need different names"),
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
            None => match self.struct_by_name.get(name) {
                Some(&id) => Type::Struct(id),
                None => {
                    self.unknown_type(name, ty.span());
                    Type::Error
                }
            },
        }
    }

    fn unknown_type(&mut self, name: &str, span: Span) {
        let mut diag = Diagnostic::error(format!("unknown type `{name}`"))
            .with_code("E0212")
            .with_primary(span, "not a known type");
        let similar_struct = suggest::similar(name, self.struct_by_name.keys().copied());
        diag = match similar_struct.or(suggest::type_name(name)) {
            Some(s) => diag.with_help(format!("did you mean `{s}`?")),
            None => diag.with_note(
                "the available types are `i64`, `bool`, `string`, `void` and the declared structs",
            ),
        };
        self.diags.push(diag);
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
                let ret = self.show(ctx.ret);
                diag = diag.with_secondary(span, format!("`{ret}` must be returned"));
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
        path: &[ast::Ident],
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
        let (var_ty, mutable, decl_span) = (local.ty, local.mutable, local.span);
        let is_param = id.0 < ctx.param_count;

        // Resolve the field path: `current` is the place being assigned, as
        // an expression (needed for compound assignment).
        let mut current = CheckedExpr {
            kind: CheckedExprKind::Local(id),
            ty: var_ty,
            span: target.span,
        };
        let mut indices = Vec::with_capacity(path.len());
        for field in path {
            let Some((index, ty)) = self.field(current.ty, field, current.span) else {
                return CheckedStmt::Expr(value);
            };
            indices.push(index);
            current = CheckedExpr {
                span: current.span.to(field.span),
                kind: CheckedExprKind::Field {
                    base: Box::new(current),
                    index,
                },
                ty,
            };
        }
        let ty = current.ty;

        if !mutable {
            let ty = self.show(var_ty);
            let message = if path.is_empty() {
                format!("cannot assign to immutable variable `{}`", target.name)
            } else {
                format!(
                    "cannot assign to a field of immutable variable `{}`",
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
                        "its fields cannot be changed"
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
            ctx.reassigned[id.0] = true;
        }

        let value = match op {
            None => value,
            Some(op) => {
                let span = target.span.to(value.span);
                self.resolve_binary(op, op_span, current, value, span)
            }
        };
        let reason = if path.is_empty() {
            "variable declared here"
        } else {
            "the variable's type is declared here"
        };
        self.expect_type(&value, ty, Some(decl_span), reason);
        CheckedStmt::Assign {
            local: id,
            path: indices,
            value,
        }
    }

    /// Looks up `field` in a value of type `ty`: its index and type.
    /// Reports an error (unless `ty` is already an error) when there is none.
    fn field(&mut self, ty: Type, field: &ast::Ident, base_span: Span) -> Option<(usize, Type)> {
        let id = match ty {
            Type::Struct(id) => id,
            Type::Error => return None,
            other => {
                let shown = self.show(other);
                self.diags.push(
                    Diagnostic::error(format!("`{shown}` has no fields"))
                        .with_code("E0224")
                        .with_primary(field.span, "unknown field")
                        .with_secondary(base_span, format!("this is `{shown}`"))
                        .with_note("only structs have fields"),
                );
                return None;
            }
        };
        let info = &self.structs[id.0];
        if let Some(index) = info.fields.iter().position(|f| f.name == field.name) {
            return Some((index, info.fields[index].ty));
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
            ast::ExprKind::StructLit { name, fields } => {
                return self.check_struct_lit(name, fields, span, ctx);
            }
            ast::ExprKind::Field { base, field } => {
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
        };
        CheckedExpr { kind, ty, span }
    }

    fn check_struct_lit(
        &mut self,
        name: &ast::Ident,
        fields: &[ast::FieldInit],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let values: Vec<CheckedExpr> = fields
            .iter()
            .map(|f| self.check_expr(&f.value, ctx))
            .collect();
        let Some(&id) = self.struct_by_name.get(name.name.as_str()) else {
            if Type::from_name(&name.name).is_some() {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is not a struct", name.name))
                        .with_code("E0212")
                        .with_primary(name.span, "built-in type"),
                );
            } else {
                self.unknown_type(&name.name, name.span);
            }
            return error_expr(span);
        };
        let mut checked: Vec<(usize, CheckedExpr)> = Vec::with_capacity(fields.len());
        let mut ok = true;
        let mut given: Vec<Option<Span>> = vec![None; self.structs[id.0].fields.len()];
        for (field, value) in fields.iter().zip(values) {
            let Some((index, ty)) = self.field(Type::Struct(id), &field.name, name.span) else {
                ok = false;
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
                continue;
            }
            given[index] = Some(field.name.span);
            let declared = self.structs[id.0].fields[index].span;
            self.expect_type(&value, ty, Some(declared), "field declared here");
            checked.push((index, value));
        }
        let missing: Vec<String> = self.structs[id.0]
            .fields
            .iter()
            .zip(&given)
            .filter(|(_, g)| g.is_none())
            .map(|(f, _)| format!("`{}`", f.name))
            .collect();
        if !missing.is_empty() {
            let info = &self.structs[id.0];
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
        CheckedExpr {
            kind: CheckedExprKind::StructLit {
                id,
                fields: checked,
            },
            ty: Type::Struct(id),
            span,
        }
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
        if self.struct_by_name.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("`{name}` is a struct, not a value"))
                    .with_code("E0214")
                    .with_primary(ident.span, "structs cannot be used as values")
                    .with_help(format!(
                        "to create a `{name}`, write `{name} {{ field: value, ... }}`"
                    )),
            );
        } else if self.by_name.contains_key(name) || brim_stdlib::lookup(name).is_some() {
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
                let ty = self.show(ty);
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
        let (l, r) = (self.show(lhs.ty), self.show(rhs.ty));
        let either_struct = matches!(lhs.ty, Type::Struct(_)) || matches!(rhs.ty, Type::Struct(_));
        let mut diag = Diagnostic::error(format!("cannot apply `{sym}` to `{l}` and `{r}`"))
            .with_code("E0209")
            .with_primary(op_span, format!("`{l} {sym} {r}` is not defined"))
            .with_secondary(lhs.span, format!("`{l}`"))
            .with_secondary(rhs.span, format!("`{r}`"));
        diag = match op {
            Op::Eq | Op::Ne if either_struct && lhs.ty == rhs.ty => diag
                .with_note(format!("structs cannot be compared with `{sym}` yet"))
                .with_help("compare their fields instead"),
            Op::Add if lhs.ty == Type::String || rhs.ty == Type::String => diag
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
            if args.iter().any(|a| a.ty == Type::Error) {
                return error_expr(span);
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

        let mut diag = Diagnostic::error(format!("undefined function `{name}`"))
            .with_code("E0201")
            .with_primary(callee.span, "not found");
        if self.struct_by_name.contains_key(name) {
            diag = diag.with_help(format!(
                "`{name}` is a struct; create one with `{name} {{ field: value, ... }}`"
            ));
            self.diags.push(diag);
            return error_expr(span);
        }
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
