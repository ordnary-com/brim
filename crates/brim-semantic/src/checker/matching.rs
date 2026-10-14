//! `match` statements: patterns, bindings, exhaustiveness and unreachable
//! arms.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx};
use crate::checked::{CheckedArm, CheckedPattern, CheckedStmt};
use crate::{Def, EnumId, Type};

/// What the arms seen so far cover.
#[derive(Default)]
struct Coverage {
    wildcard: bool,
    /// Per enum variant, or `[false, true]` for `bool`.
    variants: Vec<bool>,
    /// Literal values already matched (integers and strings).
    literals: Vec<String>,
}

/// The shape of the value being matched.
enum Scrutinee {
    Enum(EnumId, Vec<Type>),
    Bool,
    I64,
    String,
    /// A type that can only be matched with `_`.
    Other,
    Error,
}

impl<'a> Checker<'a> {
    pub(super) fn check_match(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        ctx: &mut FnCtx,
    ) -> (CheckedStmt, bool) {
        let scrutinee = self.check_expr(scrutinee, ctx);
        let shape = match scrutinee.ty {
            Type::Error => Scrutinee::Error,
            Type::Bool => Scrutinee::Bool,
            Type::I64 => Scrutinee::I64,
            Type::String => Scrutinee::String,
            ty => match self.as_enum(ty) {
                Some((e, args)) => Scrutinee::Enum(e, args),
                None => Scrutinee::Other,
            },
        };
        let mut coverage = Coverage {
            variants: match &shape {
                Scrutinee::Enum(e, _) => vec![false; self.enums[e.0].variants.len()],
                Scrutinee::Bool => vec![false; 2],
                _ => Vec::new(),
            },
            ..Coverage::default()
        };

        let mut checked_arms = Vec::with_capacity(arms.len());
        let mut all_diverge = true;
        for arm in arms {
            ctx.scopes.push(HashMap::new());
            let pattern = self.check_pattern(&arm.pattern, &shape, scrutinee.ty, ctx);
            if let Some(pattern) = &pattern {
                self.note_coverage(pattern, &mut coverage, arm.pattern.span);
            }
            let (body, diverges) = self.check_stmts(&arm.body.stmts, ctx);
            ctx.scopes.pop();
            all_diverge &= diverges;
            checked_arms.push(CheckedArm {
                pattern: pattern.unwrap_or(CheckedPattern::Wildcard),
                body,
            });
        }

        let exhaustive = coverage.wildcard
            || (!coverage.variants.is_empty() && coverage.variants.iter().all(|c| *c));
        if !exhaustive && !matches!(shape, Scrutinee::Error) {
            self.report_not_exhaustive(&shape, &coverage, scrutinee.span);
        }
        let diverges = exhaustive && !arms.is_empty() && all_diverge;
        (
            CheckedStmt::Match {
                scrutinee,
                arms: checked_arms,
            },
            diverges,
        )
    }

    /// Checks one pattern against the matched value. `None` after an error.
    fn check_pattern(
        &mut self,
        pattern: &ast::Pattern,
        shape: &Scrutinee,
        scrutinee_ty: Type,
        ctx: &mut FnCtx,
    ) -> Option<CheckedPattern> {
        let checked = self.check_pattern_inner(pattern, shape, scrutinee_ty, ctx);
        // After an error, still declare the pattern's names, so the arm's
        // body does not report them as undefined.
        if checked.is_none()
            && let ast::PatternKind::Variant { bindings, .. } = &pattern.kind
        {
            for binding in bindings {
                if binding.name != "_" {
                    self.declare(binding, Type::Error, false, ctx);
                }
            }
        }
        checked
    }

    fn check_pattern_inner(
        &mut self,
        pattern: &ast::Pattern,
        shape: &Scrutinee,
        scrutinee_ty: Type,
        ctx: &mut FnCtx,
    ) -> Option<CheckedPattern> {
        match &pattern.kind {
            ast::PatternKind::Wildcard => Some(CheckedPattern::Wildcard),
            ast::PatternKind::Literal(expr) => {
                let literal = match &expr.kind {
                    ast::ExprKind::Int(v) => i64::try_from(*v).ok().map(CheckedPattern::Int),
                    ast::ExprKind::Unary { operand, .. } => match operand.kind {
                        ast::ExprKind::Int(v) => {
                            0i64.checked_sub_unsigned(v).map(CheckedPattern::Int)
                        }
                        _ => None,
                    },
                    ast::ExprKind::Str(s) => Some(CheckedPattern::Str(s.clone())),
                    ast::ExprKind::Bool(b) => Some(CheckedPattern::Bool(*b)),
                    _ => None,
                };
                let Some(literal) = literal else {
                    self.diags.push(
                        Diagnostic::error("integer literal is out of range for `i64`")
                            .with_code("E0215")
                            .with_primary(pattern.span, ""),
                    );
                    return None;
                };
                let literal_ty = match literal {
                    CheckedPattern::Int(_) => Type::I64,
                    CheckedPattern::Str(_) => Type::String,
                    _ => Type::Bool,
                };
                if matches!(shape, Scrutinee::Error) {
                    return None;
                }
                if literal_ty != scrutinee_ty {
                    self.pattern_mismatch(pattern.span, scrutinee_ty, &self.show(literal_ty));
                    return None;
                }
                Some(literal)
            }
            ast::PatternKind::Variant {
                enum_name,
                variant,
                bindings,
            } => {
                let (e, args) = match shape {
                    Scrutinee::Enum(e, args) => (*e, args.clone()),
                    Scrutinee::Error => return None,
                    _ => {
                        let what = match enum_name {
                            Some(name) => format!("{}.{}", name.name, variant.name),
                            None => variant.name.clone(),
                        };
                        self.pattern_mismatch(pattern.span, scrutinee_ty, &format!("`{what}`"));
                        return None;
                    }
                };
                if let Some(name) = enum_name {
                    match self.type_by_name.get(&name.name) {
                        Some(Def::Enum(named)) if *named == e => {}
                        Some(_) | None => {
                            let ty = self.show(scrutinee_ty);
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "pattern for `{}` cannot match a `{ty}`",
                                    name.name
                                ))
                                .with_code("E0230")
                                .with_primary(name.span, format!("expected `{ty}`")),
                            );
                            return None;
                        }
                    }
                }
                let v = self.variant_index(e, variant)?;
                let fields = self.enums[e.0].variants[v].fields.clone();
                let variant_name = self.enums[e.0].variants[v].name.clone();
                if bindings.len() != fields.len() {
                    let placeholders = vec!["_"; fields.len()].join(", ");
                    let fix = if fields.is_empty() {
                        format!("write just `{variant_name}`")
                    } else {
                        format!(
                            "write `{variant_name}({placeholders})`, naming the values you need"
                        )
                    };
                    self.diags.push(
                        Diagnostic::error(format!(
                            "variant `{variant_name}` has {} but the pattern has {}",
                            super::plural(fields.len(), "value"),
                            bindings.len()
                        ))
                        .with_code("E0228")
                        .with_primary(pattern.span, "")
                        .with_help(fix),
                    );
                    return None;
                }
                let mut locals = Vec::with_capacity(bindings.len());
                for (binding, field) in bindings.iter().zip(fields) {
                    if binding.name == "_" {
                        locals.push(None);
                        continue;
                    }
                    let ty = self.types.subst(field, &args);
                    locals.push(Some(self.declare(binding, ty, false, ctx)));
                }
                Some(CheckedPattern::Variant {
                    index: v,
                    bindings: locals,
                })
            }
        }
    }

    fn pattern_mismatch(&mut self, span: Span, scrutinee_ty: Type, found: &str) {
        let ty = self.show(scrutinee_ty);
        self.diags.push(
            Diagnostic::error("pattern does not match the type of the value")
                .with_code("E0230")
                .with_primary(
                    span,
                    format!("expected a pattern for `{ty}`, found {found}"),
                )
                .with_note(match self.as_struct(scrutinee_ty) {
                    Some(_) => "structs can only be matched with `_`".to_string(),
                    None => format!("the value being matched is `{ty}`"),
                }),
        );
    }

    /// Records what `pattern` covers; warns when it covers nothing new.
    fn note_coverage(&mut self, pattern: &CheckedPattern, coverage: &mut Coverage, span: Span) {
        let new = match pattern {
            CheckedPattern::Wildcard => {
                let new = !coverage.wildcard;
                coverage.wildcard = true;
                new
            }
            CheckedPattern::Variant { index, .. } => {
                !std::mem::replace(&mut coverage.variants[*index], true)
            }
            CheckedPattern::Bool(b) => {
                !std::mem::replace(&mut coverage.variants[usize::from(*b)], true)
            }
            CheckedPattern::Int(v) => {
                let key = v.to_string();
                let new = !coverage.literals.contains(&key);
                coverage.literals.push(key);
                new
            }
            CheckedPattern::Str(s) => {
                let key = format!("{s:?}");
                let new = !coverage.literals.contains(&key);
                coverage.literals.push(key);
                new
            }
        };
        let after_wildcard = coverage.wildcard && !matches!(pattern, CheckedPattern::Wildcard);
        if !new || after_wildcard {
            self.diags.push(
                Diagnostic::warning("unreachable pattern")
                    .with_code("W0004")
                    .with_primary(span, "this arm will never run")
                    .with_note("an earlier arm already matches every value this pattern matches"),
            );
        }
    }

    fn report_not_exhaustive(
        &mut self,
        shape: &Scrutinee,
        coverage: &Coverage,
        scrutinee_span: Span,
    ) {
        let ty = match shape {
            Scrutinee::Enum(..) | Scrutinee::Other => "",
            Scrutinee::Bool => "bool",
            Scrutinee::I64 => "i64",
            Scrutinee::String => "string",
            Scrutinee::Error => return,
        };
        let missing: Vec<String> = match shape {
            Scrutinee::Enum(e, _) => {
                let info = &self.enums[e.0];
                info.variants
                    .iter()
                    .zip(&coverage.variants)
                    .filter(|(_, covered)| !**covered)
                    .map(|(v, _)| {
                        if v.fields.is_empty() {
                            format!("`{}`", v.name)
                        } else {
                            let blanks = vec!["_"; v.fields.len()].join(", ");
                            format!("`{}({blanks})`", v.name)
                        }
                    })
                    .collect()
            }
            Scrutinee::Bool => [false, true]
                .iter()
                .zip(&coverage.variants)
                .filter(|(_, covered)| !**covered)
                .map(|(b, _)| format!("`{b}`"))
                .collect(),
            _ => Vec::new(),
        };
        let mut diag = Diagnostic::error("`match` does not cover every value")
            .with_code("E0229")
            .with_primary(scrutinee_span, "some values have no matching arm");
        diag = if missing.is_empty() {
            let what = if ty.is_empty() {
                "this type".to_string()
            } else {
                format!("`{ty}`")
            };
            diag.with_note(format!("{what} has too many values to list each one"))
                .with_help("add a `_ => ...` arm for the remaining values")
        } else {
            diag.with_note(format!("not covered: {}", missing.join(", ")))
                .with_help("add an arm for each, or a `_ => ...` arm")
        };
        self.diags.push(diag);
    }
}
