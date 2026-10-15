//! `const NAME: type = value`: named values computed from literals.
//!
//! A constant's value is checked once, the first time it is used (so
//! constants can use each other in any order), and every use is that
//! expression again. Only literals, operators, other constants and
//! struct, enum, list, map and box values are allowed, so evaluating it
//! again has no effects.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx, error_expr};
use crate::Type;
use crate::checked::{CheckedExpr, CheckedExprKind};

pub(super) struct ConstInfo<'a> {
    pub decl: &'a ast::ConstDecl,
    pub module: usize,
    pub state: State,
}

pub(super) enum State {
    Unchecked,
    /// Being checked: using it now is a cycle.
    Checking,
    Done(CheckedExpr),
}

impl<'a> Checker<'a> {
    /// Declares module `m`'s constants.
    pub(super) fn collect_consts(&mut self, m: usize, program: &'a ast::Program) {
        for decl in &program.consts {
            let name = &decl.name.name;
            if let Some(&first) = self.scopes[m].consts.get(name) {
                let first_span = self.consts[first].decl.name.span;
                self.diags.push(
                    Diagnostic::error(format!("constant `{name}` is defined more than once"))
                        .with_code("E0202")
                        .with_primary(decl.name.span, "redefined here")
                        .with_secondary(first_span, "first defined here"),
                );
                continue;
            }
            self.scopes[m]
                .consts
                .insert(name.clone(), self.consts.len());
            self.consts.push(ConstInfo {
                decl,
                module: m,
                state: State::Unchecked,
            });
        }
    }

    /// The constant `name` of the current module, if there is one.
    pub(super) fn const_named(&self, name: &str) -> Option<usize> {
        self.scopes[self.module].consts.get(name).copied()
    }

    /// The value of constant `id`, used at `used_at` (`None` when it is
    /// only checked).
    pub(super) fn const_value(&mut self, id: usize, used_at: Option<Span>) -> CheckedExpr {
        let span = used_at.unwrap_or(self.consts[id].decl.span);
        match &self.consts[id].state {
            State::Done(value) => return value.clone(),
            State::Checking => {
                let decl = self.consts[id].decl;
                self.diags.push(
                    Diagnostic::error(format!(
                        "constant `{}` is defined in terms of itself",
                        decl.name.name
                    ))
                    .with_code("E0249")
                    .with_primary(span, "used here while its value is computed")
                    .with_secondary(decl.name.span, "defined here"),
                );
                return error_expr(span);
            }
            State::Unchecked => {}
        }
        self.consts[id].state = State::Checking;
        let decl = self.consts[id].decl;

        // Checked in its own module, outside any function.
        let current = self.module;
        self.leave();
        self.enter(self.consts[id].module);
        let type_params = std::mem::take(&mut self.type_params);
        let type_bounds = std::mem::take(&mut self.type_bounds);
        let mut ctx = FnCtx {
            scopes: vec![HashMap::new()],
            ..FnCtx::default()
        };
        let value = match &decl.ty {
            Some(ty) => {
                let declared = self.resolve_type(ty, "a constant");
                let expected = (declared != Type::Error).then_some(declared);
                let value = self.check_expr_with(&decl.value, expected, &mut ctx);
                self.expect_type(
                    &value,
                    declared,
                    Some(ty.span()),
                    "expected due to this type",
                );
                value
            }
            None => self.check_expr(&decl.value, &mut ctx),
        };
        if value.ty == Type::Void {
            self.diags.push(
                Diagnostic::error(format!("constant `{}` has no value", decl.name.name))
                    .with_code("E0213")
                    .with_primary(value.span, "this returns nothing"),
            );
        } else if let Some(span) = not_constant(&value) {
            self.diags.push(
                Diagnostic::error(format!(
                    "the value of constant `{}` must be computed from literals",
                    decl.name.name
                ))
                .with_code("E0249")
                .with_primary(span, "not allowed in a constant")
                .with_note(
                    "a constant can use literals, operators, other constants, and struct, enum, list and map values",
                )
                .with_help("compute it in a function instead"),
            );
        }
        self.type_params = type_params;
        self.type_bounds = type_bounds;
        self.leave();
        self.enter(current);

        self.consts[id].state = State::Done(value.clone());
        value
    }

    /// `module.NAME` for a `pub` constant of another module.
    pub(super) fn module_const(
        &mut self,
        target: usize,
        field: &ast::Ident,
        span: Span,
    ) -> Option<CheckedExpr> {
        let id = *self.scopes[target].consts.get(&field.name)?;
        if !self.consts[id].decl.public && target != self.module {
            let module = self.module_names[target].clone();
            self.private_item(&field.name, &module, field.span);
            return Some(error_expr(span));
        }
        Some(self.const_value(id, Some(span)))
    }
}

/// The first part of a checked value that is not allowed in a constant.
fn not_constant(expr: &CheckedExpr) -> Option<Span> {
    use CheckedExprKind as K;
    match &expr.kind {
        K::Int(_) | K::Float(_) | K::Bool(_) | K::Str(_) | K::FnRef(_) => None,
        K::Unary { operand, .. } | K::BoxNew(operand) | K::IntCast(operand) => {
            not_constant(operand)
        }
        K::Binary { lhs, rhs, .. } | K::And(lhs, rhs) | K::Or(lhs, rhs) => {
            not_constant(lhs).or_else(|| not_constant(rhs))
        }
        K::Variant { args: items, .. } | K::ListLit(items) => items.iter().find_map(not_constant),
        K::StructLit { fields } => fields.iter().find_map(|(_, v)| not_constant(v)),
        K::MapLit(entries) => entries
            .iter()
            .find_map(|(k, v)| not_constant(k).or_else(|| not_constant(v))),
        _ => Some(expr.span),
    }
}
