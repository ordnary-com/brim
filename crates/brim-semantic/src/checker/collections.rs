//! Lists: literals, indexing, methods, `for` loops, and places (the targets
//! of assignments and of methods that change a list).

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx, error_expr, plural, supplied};
use crate::checked::{
    CheckedExpr, CheckedExprKind, CheckedForIter, CheckedPlace, CheckedStmt, ListOp, ListReceiver,
    MapOp, PlaceStep,
};
use crate::{Def, Type};

/// One step of a place as written: `.field` or `[index]`.
pub(super) enum StepRef<'e> {
    Field(&'e ast::Ident),
    Index(&'e ast::Expr),
}

/// The list methods: name, operation, parameter types (`Param(0)` is the
/// element type) and result type.
fn list_method(name: &str) -> Option<(ListOp, Vec<Type>, Type)> {
    let elem = Type::Param(0);
    Some(match name {
        "len" => (ListOp::Len, vec![], Type::I64),
        "is_empty" => (ListOp::IsEmpty, vec![], Type::Bool),
        "get" => (ListOp::Get, vec![Type::I64], Type::Error), // Option[T]
        "first" => (ListOp::First, vec![], Type::Error),      // Option[T]
        "last" => (ListOp::Last, vec![], Type::Error),        // Option[T]
        "contains" => (ListOp::Contains, vec![elem], Type::Bool),
        "join" => (ListOp::Join, vec![Type::String], Type::String),
        "push" => (ListOp::Push, vec![elem], Type::Void),
        "pop" => (ListOp::Pop, vec![], Type::Error), // Option[T]
        "insert" => (ListOp::Insert, vec![Type::I64, elem], Type::Void),
        "remove" => (ListOp::Remove, vec![Type::I64], elem),
        "clear" => (ListOp::Clear, vec![], Type::Void),
        _ => return None,
    })
}

/// The map methods: name, operation, parameter types (`Param(0)` is the
/// key type, `Param(1)` the value type).
fn map_method(name: &str) -> Option<(MapOp, Vec<Type>)> {
    let (key, value) = (Type::Param(0), Type::Param(1));
    Some(match name {
        "len" => (MapOp::Len, vec![]),
        "is_empty" => (MapOp::IsEmpty, vec![]),
        "get" => (MapOp::Get, vec![key]),
        "contains_key" => (MapOp::ContainsKey, vec![key]),
        "keys" => (MapOp::Keys, vec![]),
        "values" => (MapOp::Values, vec![]),
        "insert" => (MapOp::Insert, vec![key, value]),
        "remove" => (MapOp::Remove, vec![key]),
        "clear" => (MapOp::Clear, vec![]),
        _ => return None,
    })
}

const MAP_METHODS: &[&str] = &[
    "len",
    "is_empty",
    "get",
    "contains_key",
    "keys",
    "values",
    "insert",
    "remove",
    "clear",
];

const LIST_METHODS: &[&str] = &[
    "len",
    "is_empty",
    "get",
    "first",
    "last",
    "contains",
    "join",
    "push",
    "pop",
    "insert",
    "remove",
    "clear",
    "sorted",
    "sorted_by",
    "reversed",
    "slice",
    "index_of",
];

impl<'a> Checker<'a> {
    /// The element type, if `ty` is a list.
    pub(super) fn as_list(&self, ty: Type) -> Option<Type> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        (named.def == Def::List).then(|| named.args[0])
    }

    /// The key and value types, if `ty` is a map.
    pub(super) fn as_map(&self, ty: Type) -> Option<(Type, Type)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        (named.def == Def::Map).then(|| (named.args[0], named.args[1]))
    }

    pub(super) fn check_map_lit(
        &mut self,
        entries: &[(ast::Expr, ast::Expr)],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let (mut key_ty, mut value_ty) = match expected.and_then(|t| self.as_map(t)) {
            Some((k, v)) => (Some(k), Some(v)),
            None => (None, None),
        };
        let mut checked = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let key = self.check_expr_with(key, key_ty, ctx);
            let value = self.check_expr_with(value, value_ty, ctx);
            if key_ty.is_none() && key.ty != Type::Error {
                key_ty = Some(key.ty);
            }
            if value_ty.is_none() && value.ty != Type::Error {
                value_ty = Some(value.ty);
            }
            checked.push((key, value));
        }
        let (Some(key_ty), Some(value_ty)) = (key_ty, value_ty) else {
            if entries.is_empty() && !self.quiet_inference {
                self.diags.push(
                    Diagnostic::error("cannot infer the types of an empty map")
                        .with_code("E0226")
                        .with_primary(span, "the type is not known here")
                        .with_help(
                            "add a type annotation, for example `let m: Map[string, i64] = [:]`",
                        ),
                );
            }
            return error_expr(span);
        };
        if !matches!(key_ty, Type::I64 | Type::Bool | Type::String) {
            let key = self.show(key_ty);
            self.diags.push(
                Diagnostic::error(format!("`{key}` cannot be a map key"))
                    .with_code("E0236")
                    .with_primary(checked[0].0.span, "")
                    .with_note("map keys are `i64`, `bool` or `string`"),
            );
            return error_expr(span);
        }
        for (key, value) in &checked {
            self.expect_type(key, key_ty, None, "");
            self.expect_type(value, value_ty, None, "");
        }
        CheckedExpr {
            ty: self.types.intern(Def::Map, vec![key_ty, value_ty]),
            kind: CheckedExprKind::MapLit(checked),
            span,
        }
    }

    /// `base.name(args)` where `base` is a map.
    pub(super) fn check_map_method(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let Some((op, params)) = map_method(&name.name) else {
            let receiver = self.check_expr(base, ctx);
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let ty = self.show(receiver.ty);
            let mut diag = Diagnostic::error(format!("`{ty}` has no method `{}`", name.name))
                .with_code("E0231")
                .with_primary(name.span, "unknown method");
            diag = match crate::suggest::similar(&name.name, MAP_METHODS.iter().copied()) {
                Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
                None => diag.with_note(format!(
                    "map methods: {}",
                    MAP_METHODS
                        .iter()
                        .map(|m| format!("`{m}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
            self.diags.push(diag);
            return Some(error_expr(span));
        };
        let receiver = if op.changes_map() {
            let Some(place) = self.check_place(base, ctx, &format!("call `{}` on", name.name))
            else {
                for arg in args {
                    self.check_expr(arg, ctx);
                }
                return Some(error_expr(span));
            };
            ListReceiver::Place(place)
        } else {
            ListReceiver::Value(Box::new(self.check_expr(base, ctx)))
        };
        let map_ty = match &receiver {
            ListReceiver::Value(v) => v.ty,
            ListReceiver::Place(p) => p.ty,
        };
        let (key, value) = self.as_map(map_ty).expect("checked by the caller");
        if args.len() != params.len() {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "`{}` takes {} but {} supplied",
                    name.name,
                    plural(params.len(), "argument"),
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(
                    span,
                    format!("expected {}", plural(params.len(), "argument")),
                ),
            );
            return Some(error_expr(span));
        }
        let mut checked = Vec::with_capacity(args.len());
        for (arg, param) in args.iter().zip(&params) {
            let param = self.types.subst(*param, &[key, value]);
            let arg = self.check_expr_with(arg, Some(param), ctx);
            self.expect_type(&arg, param, None, "");
            checked.push(arg);
        }
        let ty = match op {
            MapOp::Len => Type::I64,
            MapOp::IsEmpty | MapOp::ContainsKey => Type::Bool,
            MapOp::Get | MapOp::Remove => self.option_of(value),
            MapOp::Keys => self.list_of(key),
            MapOp::Values => self.list_of(value),
            MapOp::Insert | MapOp::Clear => Type::Void,
        };
        Some(CheckedExpr {
            kind: CheckedExprKind::MapOp {
                op,
                receiver,
                args: checked,
            },
            ty,
            span,
        })
    }

    pub(super) fn list_of(&mut self, elem: Type) -> Type {
        self.types.intern(Def::List, vec![elem])
    }

    fn option_of(&mut self, inner: Type) -> Type {
        self.types.intern(Def::Enum(crate::EnumId(0)), vec![inner])
    }

    pub(super) fn check_list_lit(
        &mut self,
        items: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let mut elem = expected.and_then(|t| self.as_list(t));
        let mut checked = Vec::with_capacity(items.len());
        for item in items {
            let value = self.check_expr_with(item, elem, ctx);
            if elem.is_none() && value.ty != Type::Error {
                elem = Some(value.ty);
            }
            checked.push(value);
        }
        let Some(elem) = elem else {
            if items.is_empty() && !self.quiet_inference {
                self.diags.push(
                    Diagnostic::error("cannot infer the element type of an empty list")
                        .with_code("E0226")
                        .with_primary(span, "the type is not known here")
                        .with_help("add a type annotation, for example `let xs: List[i64] = []`"),
                );
            }
            return error_expr(span);
        };
        for value in &checked {
            self.expect_type(value, elem, None, "");
        }
        CheckedExpr {
            ty: self.list_of(elem),
            kind: CheckedExprKind::ListLit(checked),
            span,
        }
    }

    pub(super) fn check_index(
        &mut self,
        base: &ast::Expr,
        index: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let base = self.check_expr(base, ctx);
        let index = self.check_expr(index, ctx);
        let Some(elem) = self.list_element(&base, &index) else {
            return error_expr(span);
        };
        CheckedExpr {
            kind: CheckedExprKind::Index {
                base: Box::new(base),
                index: Box::new(index),
            },
            ty: elem,
            span,
        }
    }

    /// Checks `base[index]` on a list or map: the element (or value) type,
    /// or `None` after an error.
    fn list_element(&mut self, base: &CheckedExpr, index: &CheckedExpr) -> Option<Type> {
        if let Some((key, value)) = self.as_map(base.ty) {
            self.expect_type(index, key, None, "");
            return Some(value);
        }
        self.expect_type(index, Type::I64, None, "");
        if base.ty == Type::Error {
            return None;
        }
        match self.as_list(base.ty) {
            Some(elem) => Some(elem),
            None => {
                let ty = self.show(base.ty);
                self.diags.push(
                    Diagnostic::error(format!("cannot index into a value of type `{ty}`"))
                        .with_code("E0234")
                        .with_primary(base.span, format!("this is `{ty}`"))
                        .with_note("only lists and maps can be indexed with `[...]`"),
                );
                None
            }
        }
    }

    /// `base.name(args)` where `base` is a list.
    pub(super) fn check_list_method(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        let Some((op, params, ret)) = list_method(&name.name) else {
            if let Some(result) = self.prelude_method("list", base, name, args, span, ctx) {
                return Some(result);
            }
            let receiver = self.check_expr(base, ctx);
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let ty = self.show(receiver.ty);
            let mut diag = Diagnostic::error(format!("`{ty}` has no method `{}`", name.name))
                .with_code("E0231")
                .with_primary(name.span, "unknown method");
            // Names other languages use for the same thing.
            let alias = match name.name.as_str() {
                "size" | "length" | "count" => Some("len"),
                "sort" => Some("sorted"),
                "reverse" => Some("reversed"),
                "append" | "add" => Some("push"),
                "find" | "position" => Some("index_of"),
                _ => None,
            };
            let similar =
                alias.or_else(|| crate::suggest::similar(&name.name, LIST_METHODS.iter().copied()));
            diag = match similar {
                Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
                None => diag.with_note(format!(
                    "list methods: {}",
                    LIST_METHODS
                        .iter()
                        .map(|m| format!("`{m}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
            self.diags.push(diag);
            return Some(error_expr(span));
        };

        let receiver = if op.changes_list() {
            let Some(place) = self.check_place(base, ctx, &format!("call `{}` on", name.name))
            else {
                for arg in args {
                    self.check_expr(arg, ctx);
                }
                return Some(error_expr(span));
            };
            ListReceiver::Place(place)
        } else {
            ListReceiver::Value(Box::new(self.check_expr(base, ctx)))
        };
        let list_ty = match &receiver {
            ListReceiver::Value(v) => v.ty,
            ListReceiver::Place(p) => p.ty,
        };
        let elem = self.as_list(list_ty).expect("checked by the caller");

        if args.len() != params.len() {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "`{}` takes {} but {} supplied",
                    name.name,
                    plural(params.len(), "argument"),
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(
                    span,
                    format!("expected {}", plural(params.len(), "argument")),
                ),
            );
            return Some(error_expr(span));
        }
        let mut checked = Vec::with_capacity(args.len());
        for (arg, param) in args.iter().zip(&params) {
            let param = self.types.subst(*param, &[elem]);
            let value = self.check_expr_with(arg, Some(param), ctx);
            self.expect_type(&value, param, None, "");
            checked.push(value);
        }
        match op {
            ListOp::Contains if !matches!(elem, Type::I64 | Type::Bool | Type::String) => {
                let ty = self.show(elem);
                self.diags.push(
                    Diagnostic::error(format!("`contains` cannot compare `{ty}` values"))
                        .with_code("E0209")
                        .with_primary(name.span, "")
                        .with_note("`contains` works on lists of `i64`, `bool` and `string`"),
                );
                return Some(error_expr(span));
            }
            ListOp::Join if elem != Type::String => {
                let ty = self.show(list_ty);
                self.diags.push(
                    Diagnostic::error(format!("`join` needs a `List[string]`, found `{ty}`"))
                        .with_code("E0206")
                        .with_primary(name.span, "")
                        .with_help("convert the elements with `to_string` first"),
                );
                return Some(error_expr(span));
            }
            _ => {}
        }
        let ty = match op {
            ListOp::Get | ListOp::First | ListOp::Last | ListOp::Pop => self.option_of(elem),
            _ => self.types.subst(ret, &[elem]),
        };
        Some(CheckedExpr {
            kind: CheckedExprKind::ListOp {
                op,
                receiver,
                args: checked,
            },
            ty,
            span,
        })
    }

    /// Resolves an expression that names a part of a variable, for changing
    /// it: `x`, `x.field`, `x[i]` and combinations. `action` completes "cannot
    /// ... immutable variable".
    pub(super) fn check_place(
        &mut self,
        expr: &ast::Expr,
        ctx: &mut FnCtx,
        action: &str,
    ) -> Option<CheckedPlace> {
        // Collect the steps from the variable outwards.
        let mut steps: Vec<StepRef<'_>> = Vec::new();
        let mut current = expr;
        let root = loop {
            match &current.kind {
                ast::ExprKind::Name(name) => break name,
                ast::ExprKind::Field { base, field } => {
                    steps.push(StepRef::Field(field));
                    current = base;
                }
                ast::ExprKind::Index { base, index } => {
                    steps.push(StepRef::Index(index));
                    current = base;
                }
                _ => {
                    self.check_expr(expr, ctx);
                    self.diags.push(
                        Diagnostic::error(format!("cannot {action} this value"))
                            .with_code("E0104")
                            .with_primary(expr.span, "not a variable")
                            .with_help("store the value in a `let mut` variable first"),
                    );
                    return None;
                }
            }
        };
        steps.reverse();
        let place = self.resolve_place(root, &steps, ctx)?;
        self.require_mutable(place.local, root, !place.steps.is_empty(), action, ctx);
        Some(place)
    }

    /// The place `root` followed by `steps`, without checking mutability.
    pub(super) fn resolve_place(
        &mut self,
        root: &ast::Ident,
        steps: &[StepRef<'_>],
        ctx: &mut FnCtx,
    ) -> Option<CheckedPlace> {
        let Some(local) = ctx.lookup(&root.name) else {
            if ctx.is_variable(&root.name) {
                self.diags.push(
                    Diagnostic::error(format!(
                        "cannot change `{}` inside a `fn` expression",
                        root.name
                    ))
                    .with_code("E0218")
                    .with_primary(root.span, "declared outside this `fn` expression")
                    .with_note("a `fn` expression works on copies of the variables it uses")
                    .with_help("return the new value from the `fn` instead, and assign it outside"),
                );
                for step in steps {
                    if let StepRef::Index(index) = step {
                        self.check_expr(index, ctx);
                    }
                }
                return None;
            }
            // Reports the undefined name (or a function used as a value).
            self.check_name(root, None, ctx);
            for step in steps {
                if let StepRef::Index(index) = step {
                    self.check_expr(index, ctx);
                }
            }
            return None;
        };
        let mut ty = ctx.locals[local.0].ty;
        let mut span = root.span;
        let mut checked_steps = Vec::with_capacity(steps.len());
        for step in steps {
            match step {
                StepRef::Field(field) => {
                    let (index, field_ty) = self.field(ty, field, span)?;
                    checked_steps.push(PlaceStep::Field(index));
                    ty = field_ty;
                    span = span.to(field.span);
                }
                StepRef::Index(index) => {
                    let index = self.check_expr(index, ctx);
                    let base = CheckedExpr {
                        kind: CheckedExprKind::Bool(false),
                        ty,
                        span,
                    };
                    let elem = self.list_element(&base, &index)?;
                    span = span.to(index.span);
                    checked_steps.push(PlaceStep::Index(index));
                    ty = elem;
                }
            }
        }
        Some(CheckedPlace {
            local,
            steps: checked_steps,
            ty,
        })
    }

    /// Reports a change to an immutable variable; otherwise records that the
    /// variable is changed.
    pub(super) fn require_mutable(
        &mut self,
        local: crate::checked::LocalId,
        name: &ast::Ident,
        partial: bool,
        action: &str,
        ctx: &mut FnCtx,
    ) {
        let info = &ctx.locals[local.0];
        if info.mutable {
            ctx.reassigned[local.0] = true;
            return;
        }
        let (decl_span, var_ty) = (info.span, info.ty);
        let is_param = local.0 < ctx.param_count;
        let ty = self.show(var_ty);
        let message = if partial {
            format!(
                "cannot {action} a part of immutable variable `{}`",
                name.name
            )
        } else {
            format!("cannot {action} immutable variable `{}`", name.name)
        };
        let mut diag = Diagnostic::error(message)
            .with_code("E0218")
            .with_primary(name.span, "declared without `mut`")
            .with_secondary(decl_span, "declared here");
        let captured = ctx.captures.iter().any(|(_, own)| *own == local);
        diag = if name.name == "self" {
            diag.with_help("declare the method with `mut self` to change `self`")
        } else if captured {
            diag.with_note("a `fn` expression works on copies of the variables it uses")
                .with_help("return the new value from the `fn` instead, and assign it outside")
        } else if is_param {
            diag.with_note("function parameters cannot be changed")
                .with_help(format!(
                    "copy it into a mutable variable: `let mut {}: {ty} = {}`",
                    name.name, name.name
                ))
        } else {
            diag.with_help(format!("make it mutable: `let mut {}`", name.name))
        };
        self.diags.push(diag);
    }

    pub(super) fn check_for(
        &mut self,
        var: &ast::Ident,
        iter: &ast::ForIter,
        body: &ast::Block,
        ctx: &mut FnCtx,
    ) -> (CheckedStmt, bool) {
        let (iter, var_ty) = match iter {
            ast::ForIter::Range(start, end) => {
                let start = self.check_expr(start, ctx);
                let end = self.check_expr(end, ctx);
                self.expect_type(&start, Type::I64, None, "");
                self.expect_type(&end, Type::I64, None, "");
                (CheckedForIter::Range(start, end), Type::I64)
            }
            ast::ForIter::Each(list) => {
                let list = self.check_expr(list, ctx);
                let elem = match self.as_list(list.ty) {
                    Some(elem) => elem,
                    None if list.ty == Type::Error => Type::Error,
                    None => {
                        let ty = self.show(list.ty);
                        let mut diag =
                            Diagnostic::error(format!("cannot loop over a value of type `{ty}`"))
                                .with_code("E0235")
                                .with_primary(list.span, format!("this is `{ty}`"))
                                .with_note("`for` loops go over a list, or a range such as `0..n`");
                        if self.as_map(list.ty).is_some() {
                            diag = diag.with_help("loop over `map.keys()` or `map.values()`");
                        }
                        self.diags.push(diag);
                        Type::Error
                    }
                };
                (CheckedForIter::Each(list), elem)
            }
        };
        ctx.scopes.push(HashMap::new());
        let var = self.declare(var, var_ty, false, ctx);
        ctx.loops.push(false);
        let (body, _) = self.check_stmts(&body.stmts, ctx);
        ctx.loops.pop();
        ctx.scopes.pop();
        // The body may run zero times, so a `for` loop never diverges.
        (CheckedStmt::For { var, iter, body }, false)
    }
}
