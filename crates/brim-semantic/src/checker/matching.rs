//! `match` statements: patterns, bindings, guards, exhaustiveness and
//! unreachable arms.
//!
//! Exhaustiveness and reachability use the usefulness algorithm of
//! Maranget ("Warnings for pattern matching", 2007): a pattern is useful
//! after some rows when some value matches it but none of the rows. An arm
//! is unreachable when its pattern is not useful after the arms above it,
//! and a `match` is exhaustive when `_` is not useful after all its arms.
//! Arms with a guard may not run, so they never cover anything.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx, LocalId};
use crate::checked::{CheckedArm, CheckedExpr, CheckedExprKind, CheckedPattern, CheckedStmt};
use crate::{Def, EnumId, Type};

/// At most this many missing patterns are listed.
const MAX_MISSING: usize = 8;

/// A pattern reduced to what matters for coverage.
#[derive(Clone, Debug)]
enum Pat {
    Wild,
    Ctor(Ctor, Vec<Pat>),
    Or(Vec<Pat>),
}

#[derive(Clone, Debug, PartialEq)]
enum Ctor {
    Variant(usize),
    Bool(bool),
    Int(i64),
    Str(String),
}

/// The constructors a type's values are built from.
enum Shape {
    Enum(EnumId, Vec<Type>),
    Bool,
    /// Too many values to list (integers, strings), or a type that can only
    /// be matched with `_` or a name.
    Open,
}

/// The names bound by the pattern being checked.
#[derive(Default)]
struct Bindings {
    /// Names bound so far, in order.
    bound: Vec<(String, LocalId)>,
    /// In the second and later alternatives of an or-pattern: the locals of
    /// the first alternative, which the same names must reuse.
    reuse: Option<HashMap<String, LocalId>>,
}

impl<'a> Checker<'a> {
    pub(super) fn check_match(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        ctx: &mut FnCtx,
    ) -> (CheckedStmt, bool) {
        let (scrutinee, arms, diverges, _) = self.match_parts(scrutinee, arms, None, ctx);
        (CheckedStmt::Match { scrutinee, arms }, diverges)
    }

    /// A `match` used as a value of type `expected` (if known): its value,
    /// and whether every arm returns or breaks instead of giving one.
    pub(super) fn check_match_value(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expected: Option<Type>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> (CheckedExpr, bool) {
        let (scrutinee, arms, diverges, ty) =
            self.match_parts(scrutinee, arms, Some(expected), ctx);
        (
            CheckedExpr {
                kind: CheckedExprKind::MatchValue {
                    scrutinee: Box::new(scrutinee),
                    arms,
                },
                ty: ty.unwrap_or(Type::Void),
                span,
            },
            diverges,
        )
    }

    /// Checks a `match`. With `value` (the expected type, if known), each
    /// arm's body ends with a value; returns their common type.
    fn match_parts(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        value: Option<Option<Type>>,
        ctx: &mut FnCtx,
    ) -> (CheckedExpr, Vec<CheckedArm>, bool, Option<Type>) {
        let scrutinee = self.check_expr(scrutinee, ctx);
        let ty = scrutinee.ty;
        let mut value_ty = value.flatten();

        let mut checked_arms = Vec::with_capacity(arms.len());
        let mut all_diverge = true;
        let mut errors = ty == Type::Error;
        for arm in arms {
            ctx.scopes.push(HashMap::new());
            let mut bindings = Bindings::default();
            let pattern = self.check_pattern(&arm.pattern, ty, &mut bindings, ctx);
            errors |= pattern.is_none();
            let guard = arm
                .guard
                .as_ref()
                .map(|guard| self.check_condition(guard, "if", ctx));
            let (body, arm_value, diverges) = if value.is_some() {
                let (body, arm_value, diverges) =
                    self.check_value_block(&arm.body.stmts, value_ty, arm.body.span, ctx);
                if let Some(v) = &arm_value {
                    match value_ty {
                        Some(expected) => self.expect_type(v, expected, None, ""),
                        None => value_ty = Some(v.ty),
                    }
                }
                (body, arm_value, diverges)
            } else {
                let (body, diverges) = self.check_stmts(&arm.body.stmts, ctx);
                (body, None, diverges)
            };
            ctx.scopes.pop();
            all_diverge &= diverges;
            checked_arms.push(CheckedArm {
                pattern: pattern.unwrap_or(CheckedPattern::Wildcard),
                guard,
                body,
                value: arm_value,
            });
        }

        // After an error the patterns may not mean what they say: report
        // nothing more about them.
        let exhaustive = errors || self.check_coverage(arms, &checked_arms, ty, scrutinee.span);
        let diverges = exhaustive && !arms.is_empty() && all_diverge;
        (scrutinee, checked_arms, diverges, value_ty)
    }

    /// Checks one pattern against a value of type `ty`. `None` after an
    /// error; the names the pattern binds are declared either way, so the
    /// arm's body does not report them as undefined.
    fn check_pattern(
        &mut self,
        pattern: &ast::Pattern,
        ty: Type,
        bindings: &mut Bindings,
        ctx: &mut FnCtx,
    ) -> Option<CheckedPattern> {
        match &pattern.kind {
            ast::PatternKind::Wildcard => Some(CheckedPattern::Wildcard),
            ast::PatternKind::Binding(name) => self.check_name_pattern(name, ty, bindings, ctx),
            ast::PatternKind::Literal(expr) => self.check_literal_pattern(pattern, expr, ty),
            ast::PatternKind::Variant {
                enum_name,
                variant,
                fields,
            } => {
                let checked = self.check_variant_pattern(pattern, enum_name.as_ref(), variant, ty);
                let Some((index, field_types)) = checked else {
                    for field in fields {
                        self.check_pattern(field, Type::Error, bindings, ctx);
                    }
                    return None;
                };
                if fields.len() != field_types.len() {
                    self.wrong_field_count(pattern.span, ty, index, fields.len());
                    for field in fields {
                        self.check_pattern(field, Type::Error, bindings, ctx);
                    }
                    return None;
                }
                let mut checked_fields = Some(Vec::with_capacity(fields.len()));
                for (field, field_ty) in fields.iter().zip(field_types) {
                    let checked = self.check_pattern(field, field_ty, bindings, ctx);
                    checked_fields = match (checked_fields, checked) {
                        (Some(mut all), Some(one)) => {
                            all.push((one, field_ty));
                            Some(all)
                        }
                        _ => None,
                    };
                }
                Some(CheckedPattern::Variant {
                    index,
                    fields: checked_fields?,
                })
            }
            ast::PatternKind::Or(alternatives) => {
                let start = bindings.bound.len();
                let first = self.check_pattern(&alternatives[0], ty, bindings, ctx);
                let first_names: Vec<(String, LocalId)> = bindings.bound[start..].to_vec();
                let mut ok = first.is_some();
                let mut checked = vec![first.unwrap_or(CheckedPattern::Wildcard)];
                let outer = bindings
                    .reuse
                    .replace(first_names.iter().cloned().collect());
                for alternative in &alternatives[1..] {
                    let before = bindings.bound.len();
                    let one = self.check_pattern(alternative, ty, bindings, ctx);
                    ok &= one.is_some();
                    checked.push(one.unwrap_or(CheckedPattern::Wildcard));
                    let names: Vec<String> = bindings
                        .bound
                        .drain(before..)
                        .map(|(name, _)| name)
                        .collect();
                    for (name, _) in &first_names {
                        if !names.contains(name) {
                            self.diags
                                .push(not_bound_everywhere(name, alternative.span));
                            ok = false;
                        }
                    }
                }
                bindings.reuse = outer;
                ok.then_some(CheckedPattern::Or(checked))
            }
        }
    }

    /// A bare name: a variant without values of the matched enum, or else a
    /// binding. Capitalized names are always meant as variants.
    fn check_name_pattern(
        &mut self,
        name: &ast::Ident,
        ty: Type,
        bindings: &mut Bindings,
        ctx: &mut FnCtx,
    ) -> Option<CheckedPattern> {
        let as_enum = self.as_enum(ty);
        if let Some((e, _)) = &as_enum
            && let Some(index) = self.enums[e.0]
                .variants
                .iter()
                .position(|v| v.name == name.name)
        {
            if !self.enums[e.0].variants[index].fields.is_empty() {
                self.wrong_field_count(name.span, ty, index, 0);
                return None;
            }
            return Some(CheckedPattern::Variant {
                index,
                fields: Vec::new(),
            });
        }
        if name.name.starts_with(|c: char| c.is_ascii_uppercase()) {
            match as_enum {
                Some((e, _)) => {
                    self.variant_index(e, name);
                }
                None if ty == Type::Error => {}
                None => self.pattern_mismatch(name.span, ty, &format!("`{}`", name.name)),
            }
            return None;
        }

        if let Some(reuse) = &bindings.reuse {
            // A later alternative of an or-pattern: the name must be bound
            // by the first alternative too, with the same type.
            let Some(&local) = reuse.get(&name.name) else {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is not bound in every alternative", name.name))
                        .with_code("E0245")
                        .with_primary(name.span, "the first alternative does not bind this")
                        .with_note("the arm can only use names that every alternative binds"),
                );
                return None;
            };
            bindings.bound.push((name.name.clone(), local));
            let first_ty = ctx.locals[local.0].ty;
            if first_ty != ty && first_ty != Type::Error && ty != Type::Error {
                let (first, here) = (self.show(first_ty), self.show(ty));
                self.diags.push(
                    Diagnostic::error(format!(
                        "`{}` has a different type in each alternative",
                        name.name
                    ))
                    .with_code("E0245")
                    .with_primary(name.span, format!("`{here}` here"))
                    .with_secondary(ctx.locals[local.0].span, format!("`{first}` here")),
                );
                return None;
            }
            return Some(CheckedPattern::Binding(local));
        }
        let local = self.declare(name, ty, false, ctx);
        bindings.bound.push((name.name.clone(), local));
        Some(CheckedPattern::Binding(local))
    }

    fn check_literal_pattern(
        &mut self,
        pattern: &ast::Pattern,
        expr: &ast::Expr,
        ty: Type,
    ) -> Option<CheckedPattern> {
        // An integer pattern takes the matched integer type.
        let int_ty = if ty.is_int() { ty } else { Type::I64 };
        let int = |magnitude, negative| {
            super::exprs::int_literal(magnitude, negative, int_ty).map(CheckedPattern::Int)
        };
        let literal = match &expr.kind {
            ast::ExprKind::Int(v) => int(*v, false),
            ast::ExprKind::Unary { operand, .. } => match operand.kind {
                ast::ExprKind::Int(v) => int(v, true),
                _ => None,
            },
            ast::ExprKind::Str(s) => Some(CheckedPattern::Str(s.clone())),
            ast::ExprKind::Bool(b) => Some(CheckedPattern::Bool(*b)),
            _ => None,
        };
        let Some(literal) = literal else {
            self.diags.push(
                Diagnostic::error(format!("integer literal is out of range for `{int_ty}`"))
                    .with_code("E0215")
                    .with_primary(pattern.span, ""),
            );
            return None;
        };
        let literal_ty = match literal {
            CheckedPattern::Int(_) => int_ty,
            CheckedPattern::Str(_) => Type::String,
            _ => Type::Bool,
        };
        if ty == Type::Error {
            return None;
        }
        if literal_ty != ty {
            self.pattern_mismatch(pattern.span, ty, &self.show(literal_ty));
            return None;
        }
        Some(literal)
    }

    /// Resolves `Enum.Variant(...)` or `Variant(...)` against the matched
    /// type: the variant's index and the types of its values.
    fn check_variant_pattern(
        &mut self,
        pattern: &ast::Pattern,
        enum_name: Option<&ast::Ident>,
        variant: &ast::Ident,
        ty: Type,
    ) -> Option<(usize, Vec<Type>)> {
        let Some((e, args)) = self.as_enum(ty) else {
            if ty != Type::Error {
                let what = match enum_name {
                    Some(name) => format!("{}.{}", name.name, variant.name),
                    None => variant.name.clone(),
                };
                self.pattern_mismatch(pattern.span, ty, &format!("`{what}`"));
            }
            return None;
        };
        if let Some(name) = enum_name {
            match self.type_by_name.get(&name.name) {
                Some(Def::Enum(named)) if *named == e => {}
                Some(_) | None => {
                    let shown = self.show(ty);
                    self.diags.push(
                        Diagnostic::error(format!(
                            "pattern for `{}` cannot match a `{shown}`",
                            name.name
                        ))
                        .with_code("E0230")
                        .with_primary(name.span, format!("expected `{shown}`")),
                    );
                    return None;
                }
            }
        }
        let index = self.variant_index(e, variant)?;
        let fields = self.enums[e.0].variants[index].fields.clone();
        let field_types = fields
            .into_iter()
            .map(|field| self.types.subst(field, &args))
            .collect();
        Some((index, field_types))
    }

    fn wrong_field_count(&mut self, span: Span, ty: Type, index: usize, found: usize) {
        let (e, _) = self.as_enum(ty).expect("only enums have variants");
        let variant = &self.enums[e.0].variants[index];
        let expected = variant.fields.len();
        let fix = if expected == 0 {
            format!("write just `{}`", variant.name)
        } else {
            let placeholders = vec!["_"; expected].join(", ");
            format!(
                "write `{}({placeholders})`, naming the values you need",
                variant.name
            )
        };
        let message = format!(
            "variant `{}` has {} but the pattern has {found}",
            variant.name,
            super::plural(expected, "value"),
        );
        self.diags.push(
            Diagnostic::error(message)
                .with_code("E0228")
                .with_primary(span, "")
                .with_help(fix),
        );
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
                    Some(_) => "structs can only be matched with `_` or a name".to_string(),
                    None => format!("the value being matched is `{ty}`"),
                }),
        );
    }

    // ---- coverage -----------------------------------------------------

    /// Warns about arms that can never run and reports values no arm
    /// matches. Returns whether every value is matched.
    fn check_coverage(
        &mut self,
        arms: &[ast::MatchArm],
        checked: &[CheckedArm],
        ty: Type,
        scrutinee_span: Span,
    ) -> bool {
        let mut rows: Vec<Vec<Pat>> = Vec::new();
        for (arm, checked) in arms.iter().zip(checked) {
            let pat = to_pat(&checked.pattern);
            if !self.useful(&rows, std::slice::from_ref(&pat), &[ty]) {
                self.diags.push(
                    Diagnostic::warning("unreachable pattern")
                        .with_code("W0004")
                        .with_primary(arm.pattern.span, "this arm will never run")
                        .with_note(
                            "an earlier arm already matches every value this pattern matches",
                        ),
                );
            } else if let (ast::PatternKind::Or(alternatives), Pat::Or(pats)) =
                (&arm.pattern.kind, &pat)
            {
                // Each alternative must match something the ones before it
                // do not.
                let mut seen = rows.clone();
                for (alternative, pat) in alternatives.iter().zip(pats) {
                    if !self.useful(&seen, std::slice::from_ref(pat), &[ty]) {
                        self.diags.push(
                            Diagnostic::warning("unreachable pattern")
                                .with_code("W0004")
                                .with_primary(alternative.span, "this alternative never matches")
                                .with_note("earlier patterns already match every value it matches"),
                        );
                    }
                    seen.push(vec![pat.clone()]);
                }
            }
            if checked.guard.is_none() {
                rows.push(vec![pat]);
            }
        }

        let missing = self.missing(&rows, &[ty], true);
        if missing.is_empty() {
            return true;
        }
        self.report_not_exhaustive(&missing, ty, scrutinee_span);
        false
    }

    fn shape(&self, ty: Type) -> Shape {
        match ty {
            Type::Bool => Shape::Bool,
            ty => match self.as_enum(ty) {
                Some((e, args)) => Shape::Enum(e, args),
                None => Shape::Open,
            },
        }
    }

    /// The types of the values inside a constructor of `ty`.
    fn ctor_fields(&mut self, ty: Type, ctor: &Ctor) -> Vec<Type> {
        match (ctor, self.shape(ty)) {
            (Ctor::Variant(index), Shape::Enum(e, args)) => {
                let fields = self.enums[e.0].variants[*index].fields.clone();
                fields
                    .into_iter()
                    .map(|field| self.types.subst(field, &args))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Every constructor of `ty`, when there are few enough to list.
    fn all_ctors(&self, ty: Type) -> Option<Vec<Ctor>> {
        match self.shape(ty) {
            Shape::Enum(e, _) => Some(
                (0..self.enums[e.0].variants.len())
                    .map(Ctor::Variant)
                    .collect(),
            ),
            Shape::Bool => Some(vec![Ctor::Bool(false), Ctor::Bool(true)]),
            Shape::Open => None,
        }
    }

    /// Whether some value matches `q` but none of `rows`. All rows and `q`
    /// have one pattern per entry of `tys`.
    fn useful(&mut self, rows: &[Vec<Pat>], q: &[Pat], tys: &[Type]) -> bool {
        let Some((head, rest)) = q.split_first() else {
            return rows.is_empty();
        };
        let rows = expand_or(rows);
        match head {
            Pat::Or(alternatives) => alternatives.iter().any(|alternative| {
                let q = prepend(vec![alternative.clone()], rest);
                self.useful(&rows, &q, tys)
            }),
            Pat::Ctor(ctor, fields) => {
                let field_tys = self.ctor_fields(tys[0], ctor);
                let rows = specialize(&rows, ctor, fields.len());
                let q = prepend(fields.clone(), rest);
                self.useful(&rows, &q, &prepend(field_tys, &tys[1..]))
            }
            Pat::Wild => match self.complete(&rows, tys[0]) {
                Some(ctors) => ctors.iter().any(|ctor| {
                    let field_tys = self.ctor_fields(tys[0], ctor);
                    let rows = specialize(&rows, ctor, field_tys.len());
                    let q = prepend(vec![Pat::Wild; field_tys.len()], rest);
                    self.useful(&rows, &q, &prepend(field_tys, &tys[1..]))
                }),
                None => self.useful(&default_rows(&rows), rest, &tys[1..]),
            },
        }
    }

    /// The constructors of `ty`, when the heads of `rows` use every one.
    fn complete(&self, rows: &[Vec<Pat>], ty: Type) -> Option<Vec<Ctor>> {
        let all = self.all_ctors(ty)?;
        let used = head_ctors(rows);
        all.iter().all(|c| used.contains(c)).then_some(all)
    }

    /// Example values that no row matches, as one pattern per entry of
    /// `tys`. `top` lists every missing variant of the matched value itself,
    /// even when no arm names one.
    fn missing(&mut self, rows: &[Vec<Pat>], tys: &[Type], top: bool) -> Vec<Vec<Pat>> {
        let Some((&ty, rest)) = tys.split_first() else {
            return if rows.is_empty() {
                vec![Vec::new()]
            } else {
                Vec::new()
            };
        };
        let rows = expand_or(rows);
        let mut out = Vec::new();
        if let Some(ctors) = self.complete(&rows, ty) {
            for ctor in ctors {
                let field_tys = self.ctor_fields(ty, &ctor);
                let arity = field_tys.len();
                let specialized = specialize(&rows, &ctor, arity);
                for mut values in self.missing(&specialized, &prepend(field_tys, rest), false) {
                    let tail = values.split_off(arity);
                    out.push(prepend(vec![Pat::Ctor(ctor.clone(), values)], &tail));
                }
                if out.len() > MAX_MISSING {
                    break;
                }
            }
            return out;
        }
        let tails = self.missing(&default_rows(&rows), rest, false);
        if tails.is_empty() {
            return out;
        }
        let used = head_ctors(&rows);
        let heads = match self.all_ctors(ty) {
            Some(all) if top || !used.is_empty() => all
                .into_iter()
                .filter(|c| !used.contains(c))
                .map(|ctor| {
                    let arity = self.ctor_fields(ty, &ctor).len();
                    Pat::Ctor(ctor, vec![Pat::Wild; arity])
                })
                .collect(),
            _ => vec![Pat::Wild],
        };
        for head in heads {
            for tail in &tails {
                out.push(prepend(vec![head.clone()], tail));
            }
        }
        out
    }

    fn report_not_exhaustive(&mut self, missing: &[Vec<Pat>], ty: Type, scrutinee_span: Span) {
        let mut diag = Diagnostic::error("`match` does not cover every value")
            .with_code("E0229")
            .with_primary(scrutinee_span, "some values have no matching arm");
        diag = if let [only] = missing
            && matches!(only[0], Pat::Wild)
        {
            let what = match ty {
                Type::I64 | Type::String => format!("`{}`", self.show(ty)),
                _ => "this type".to_string(),
            };
            diag.with_note(format!("{what} has too many values to list each one"))
                .with_help("add a `_ => ...` arm for the remaining values")
        } else {
            let mut shown: Vec<String> = missing
                .iter()
                .take(MAX_MISSING)
                .map(|values| format!("`{}`", self.show_pat(&values[0], ty)))
                .collect();
            if missing.len() > MAX_MISSING {
                shown.push("...".to_string());
            }
            diag.with_note(format!("not covered: {}", shown.join(", ")))
                .with_help("add an arm for each, or a `_ => ...` arm")
        };
        self.diags.push(diag);
    }

    fn show_pat(&mut self, pat: &Pat, ty: Type) -> String {
        match pat {
            Pat::Wild => "_".to_string(),
            Pat::Or(_) => unreachable!("examples have no alternatives"),
            Pat::Ctor(Ctor::Bool(b), _) => b.to_string(),
            Pat::Ctor(Ctor::Int(v), _) => v.to_string(),
            Pat::Ctor(Ctor::Str(s), _) => format!("{s:?}"),
            Pat::Ctor(ctor @ Ctor::Variant(index), fields) => {
                let field_tys = self.ctor_fields(ty, ctor);
                let Shape::Enum(e, _) = self.shape(ty) else {
                    unreachable!("variants belong to enums")
                };
                let name = self.enums[e.0].variants[*index].name.clone();
                if fields.is_empty() {
                    return name;
                }
                let inner: Vec<String> = fields
                    .iter()
                    .zip(field_tys)
                    .map(|(field, field_ty)| self.show_pat(field, field_ty))
                    .collect();
                format!("{name}({})", inner.join(", "))
            }
        }
    }
}

fn not_bound_everywhere(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(format!("`{name}` is not bound in every alternative"))
        .with_code("E0245")
        .with_primary(span, format!("this alternative does not bind `{name}`"))
        .with_note("the arm can only use names that every alternative binds")
}

fn to_pat(pattern: &CheckedPattern) -> Pat {
    match pattern {
        CheckedPattern::Wildcard | CheckedPattern::Binding(_) => Pat::Wild,
        CheckedPattern::Variant { index, fields } => Pat::Ctor(
            Ctor::Variant(*index),
            fields.iter().map(|(field, _)| to_pat(field)).collect(),
        ),
        CheckedPattern::Int(v) => Pat::Ctor(Ctor::Int(*v), Vec::new()),
        CheckedPattern::Str(s) => Pat::Ctor(Ctor::Str(s.clone()), Vec::new()),
        CheckedPattern::Bool(b) => Pat::Ctor(Ctor::Bool(*b), Vec::new()),
        CheckedPattern::Or(alternatives) => Pat::Or(alternatives.iter().map(to_pat).collect()),
    }
}

fn prepend<T: Clone>(mut front: Vec<T>, rest: &[T]) -> Vec<T> {
    front.extend_from_slice(rest);
    front
}

/// Splits rows whose first pattern has alternatives into one row each.
fn expand_or(rows: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match row.split_first() {
            Some((Pat::Or(alternatives), rest)) => {
                let split: Vec<Vec<Pat>> = alternatives
                    .iter()
                    .map(|alternative| prepend(vec![alternative.clone()], rest))
                    .collect();
                out.extend(expand_or(&split));
            }
            _ => out.push(row.clone()),
        }
    }
    out
}

/// The constructors the first patterns of `rows` name.
fn head_ctors(rows: &[Vec<Pat>]) -> Vec<Ctor> {
    let mut out = Vec::new();
    for row in rows {
        if let Some(Pat::Ctor(ctor, _)) = row.first()
            && !out.contains(ctor)
        {
            out.push(ctor.clone());
        }
    }
    out
}

/// The rows that match a value built with `ctor`, with that value replaced
/// by its `arity` fields.
fn specialize(rows: &[Vec<Pat>], ctor: &Ctor, arity: usize) -> Vec<Vec<Pat>> {
    rows.iter()
        .filter_map(|row| match &row[0] {
            Pat::Wild => Some(prepend(vec![Pat::Wild; arity], &row[1..])),
            Pat::Ctor(c, fields) if c == ctor => Some(prepend(fields.clone(), &row[1..])),
            Pat::Ctor(..) => None,
            Pat::Or(_) => unreachable!("alternatives are expanded first"),
        })
        .collect()
}

/// The rows that match any value whose constructor no row names.
fn default_rows(rows: &[Vec<Pat>]) -> Vec<Vec<Pat>> {
    rows.iter()
        .filter(|row| matches!(row[0], Pat::Wild))
        .map(|row| row[1..].to_vec())
        .collect()
}
