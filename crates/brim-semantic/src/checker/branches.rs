//! `if` and `match` used as values: `let sign = if n < 0 { -1 } else { 1 }`.
//!
//! A block used as a value ends with it: its last statement is an
//! expression, or an `if` or `match` that is itself a value. A block that
//! returns or breaks before its end needs no value.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx};
use crate::Type;
use crate::checked::{CheckedExpr, CheckedExprKind, CheckedStmt};

impl<'a> Checker<'a> {
    /// The statements of a block used as a value, and that value (`None`
    /// when the block diverges). The last return value tells whether it
    /// diverges.
    pub(super) fn check_value_block(
        &mut self,
        stmts: &[ast::Stmt],
        expected: Option<Type>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> (Vec<CheckedStmt>, Option<CheckedExpr>, bool) {
        ctx.scopes.push(HashMap::new());
        let result = self.value_block_inner(stmts, expected, span, ctx);
        ctx.scopes.pop();
        result
    }

    fn value_block_inner(
        &mut self,
        stmts: &[ast::Stmt],
        expected: Option<Type>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> (Vec<CheckedStmt>, Option<CheckedExpr>, bool) {
        let Some((last, init)) = stmts.split_last() else {
            self.no_value(span, "this block is empty");
            return (Vec::new(), None, false);
        };
        let (mut body, diverges) = self.check_stmts(init, ctx);
        if diverges {
            let (rest, _) = self.check_stmts(std::slice::from_ref(last), ctx);
            body.extend(rest);
            return (body, None, true);
        }
        match &last.kind {
            ast::StmtKind::Expr(expr) => {
                let value = self.check_expr_with(expr, expected, ctx);
                (body, Some(value), false)
            }
            ast::StmtKind::If { .. } | ast::StmtKind::Match { .. } => {
                let (value, diverges) = self.check_branch_value(last, expected, ctx);
                if diverges {
                    // Its branches all return or break: keep it as a
                    // statement, whose value is never needed.
                    body.push(CheckedStmt::Expr(value));
                    (body, None, true)
                } else {
                    (body, Some(value), false)
                }
            }
            _ => {
                let (stmt, diverges) = self.check_stmt(last, ctx);
                body.push(stmt);
                if !diverges {
                    self.no_value(last.span, "this statement has no value");
                }
                (body, None, diverges)
            }
        }
    }

    fn no_value(&mut self, span: Span, label: &str) {
        self.diags.push(
            Diagnostic::error("this block has no value")
                .with_code("E0250")
                .with_primary(span, label)
                .with_note(
                    "a block used as a value ends with an expression, such as `x + 1` or a call",
                ),
        );
    }

    /// An `if` statement with `else`, or a `match` statement, as a value.
    /// Returns the value, and whether every branch diverges.
    pub(super) fn check_branch_value(
        &mut self,
        stmt: &ast::Stmt,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> (CheckedExpr, bool) {
        match &stmt.kind {
            ast::StmtKind::Match { scrutinee, arms } => {
                self.check_match_value(scrutinee, arms, expected, stmt.span, ctx)
            }
            ast::StmtKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                let cond = self.check_condition(cond, "if", ctx);
                let (then_body, then_value, then_diverges) =
                    self.check_value_block(&then_block.stmts, expected, then_block.span, ctx);
                let mut ty = expected.or(then_value.as_ref().map(|v| v.ty));
                if let (Some(expected), Some(v)) = (expected, &then_value) {
                    self.expect_type(v, expected, None, "");
                }
                let (else_body, else_value, else_diverges) = match else_branch.as_deref() {
                    None => {
                        self.diags.push(
                            Diagnostic::error("`if` used as a value needs an `else`")
                                .with_code("E0250")
                                .with_primary(stmt.span, "no value when the condition is false")
                                .with_help("add an `else` block with the other value"),
                        );
                        (Vec::new(), None, false)
                    }
                    Some(ast::Stmt {
                        kind: ast::StmtKind::Block(block),
                        ..
                    }) => self.check_value_block(&block.stmts, ty, block.span, ctx),
                    // `else if`: a block of one `if`.
                    Some(other) => {
                        self.check_value_block(std::slice::from_ref(other), ty, other.span, ctx)
                    }
                };
                if let Some(v) = &else_value {
                    match ty {
                        Some(ty) => self.expect_type(v, ty, None, ""),
                        None => ty = Some(v.ty),
                    }
                }
                (
                    CheckedExpr {
                        kind: CheckedExprKind::IfValue {
                            cond: Box::new(cond),
                            then_body,
                            then_value: then_value.map(Box::new),
                            else_body,
                            else_value: else_value.map(Box::new),
                        },
                        ty: ty.unwrap_or(Type::Void),
                        span: stmt.span,
                    },
                    then_diverges && else_diverges,
                )
            }
            _ => unreachable!("only `if` and `match` are branches"),
        }
    }
}
