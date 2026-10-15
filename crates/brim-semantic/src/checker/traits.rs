//! Traits: declarations, `impl Trait for Type`, bounds on type parameters
//! and method calls through those bounds.
//!
//! Generic functions are instantiated per type, so a call through a bound
//! (`item.area()` with `T: Shape`) becomes a call of the concrete type's
//! method when the program is lowered. The built-in traits `Eq` and `Show`
//! are implemented by every type whose values can be compared and shown
//! (see `values.rs`); they allow `==` and `print` in generic code.

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, FnCtx, error_expr};
use crate::checked::{CheckedExpr, CheckedExprKind};
use crate::{Def, Type};

/// Index into the checker's traits. The built-in traits come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TraitId(pub usize);

/// `T: Eq`: `==` and `!=` work on `T`.
pub(super) const EQ: TraitId = TraitId(0);
/// `T: Show`: `print` and `to_string` work on `T`.
pub(super) const SHOW: TraitId = TraitId(1);
/// `T: Ord`: `<`, `<=`, `>` and `>=` work on `T`.
pub(super) const ORD: TraitId = TraitId(2);
/// `T: Key`: `T` can be the key of a `Map` and the element of a `Set`.
pub(super) const KEY: TraitId = TraitId(3);
/// `T: Data`: `T` can be turned into JSON and back (`std.json`'s `encode`
/// and `decode`). Every type but functions, like `Eq`.
pub(super) const DATA: TraitId = TraitId(4);
/// The last built-in trait; the program's own traits come after it.
const LAST_BUILTIN: TraitId = DATA;

pub(super) struct TraitInfo {
    pub name: String,
    pub name_span: Option<Span>,
    /// `None` for the built-in traits.
    pub module: Option<usize>,
    pub public: bool,
    pub methods: Vec<TraitMethodInfo>,
}

/// A trait method's signature. `Type::Param(0)` is `Self`.
#[derive(Clone)]
pub(super) struct TraitMethodInfo {
    pub name: String,
    pub span: Span,
    pub params: Vec<Type>,
    pub ret: Type,
}

pub(super) fn builtin_traits() -> Vec<TraitInfo> {
    ["Eq", "Show", "Ord", "Key", "Data"]
        .into_iter()
        .map(|name| TraitInfo {
            name: name.into(),
            name_span: None,
            module: None,
            public: true,
            methods: Vec::new(),
        })
        .collect()
}

impl<'a> Checker<'a> {
    /// Declares a module's traits and resolves their method signatures.
    pub(super) fn collect_traits(&mut self, program: &'a ast::Program) {
        for decl in &program.traits {
            if let Some(&first) = self.trait_by_name.get(&decl.name.name) {
                let mut diag = Diagnostic::error(format!(
                    "trait `{}` is defined more than once",
                    decl.name.name
                ))
                .with_code("E0246")
                .with_primary(decl.name.span, "redefined here");
                if let Some(span) = self.traits[first.0].name_span {
                    diag = diag.with_secondary(span, "first defined here");
                }
                self.diags.push(diag);
                continue;
            }
            self.type_params = vec!["Self".into()];
            let mut methods: Vec<TraitMethodInfo> = Vec::new();
            for method in &decl.methods {
                match &method.receiver {
                    Some(receiver) if receiver.mutable => {
                        self.diags.push(
                            Diagnostic::error("trait methods take `self`, not `mut self`")
                                .with_code("E0246")
                                .with_primary(receiver.span, ""),
                        );
                    }
                    Some(_) => {}
                    None => {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "trait method `{}` must take `self` first",
                                method.name.name
                            ))
                            .with_code("E0246")
                            .with_primary(method.name.span, "")
                            .with_note("trait methods are called on a value: `value.method()`"),
                        );
                    }
                }
                if methods.iter().any(|m| m.name == method.name.name) {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "method `{}` is declared more than once in the trait",
                            method.name.name
                        ))
                        .with_code("E0246")
                        .with_primary(method.name.span, ""),
                    );
                    continue;
                }
                let params = method
                    .params
                    .iter()
                    .map(|p| self.resolve_type(&p.ty, "a parameter"))
                    .collect();
                let ret = match &method.ret {
                    Some(ty) => self.resolve_type(ty, "return"),
                    None => Type::Void,
                };
                methods.push(TraitMethodInfo {
                    name: method.name.name.clone(),
                    span: method.span,
                    params,
                    ret,
                });
            }
            self.type_params.clear();
            let id = TraitId(self.traits.len());
            self.traits.push(TraitInfo {
                name: decl.name.name.clone(),
                name_span: Some(decl.name.span),
                module: Some(self.module),
                public: decl.public,
                methods,
            });
            self.trait_by_name.insert(decl.name.name.clone(), id);
        }
    }

    /// The trait a bound or `impl` names; `None` after reporting why not.
    pub(super) fn resolve_trait(&mut self, name: &ast::Ident) -> Option<TraitId> {
        // `module.Trait` names the same global trait.
        let short = name.name.rsplit('.').next().unwrap_or(&name.name);
        let Some(&id) = self.trait_by_name.get(short) else {
            let mut diag = Diagnostic::error(format!("unknown trait `{}`", name.name))
                .with_code("E0246")
                .with_primary(name.span, "not a known trait");
            let names: Vec<&str> = self.trait_by_name.keys().map(String::as_str).collect();
            if let Some(similar) = crate::suggest::similar(short, names) {
                diag = diag.with_help(format!("did you mean `{similar}`?"));
            }
            self.diags.push(diag);
            return None;
        };
        let info = &self.traits[id.0];
        if let Some(module) = info.module
            && module != self.module
            && !info.public
        {
            let module_name = self.module_names[module].clone();
            self.private_item(short, &module_name, name.span);
        }
        Some(id)
    }

    /// The traits each written bound names.
    pub(super) fn resolve_bounds(&mut self, bounds: &[Vec<ast::Ident>]) -> Vec<Vec<TraitId>> {
        bounds
            .iter()
            .map(|traits| {
                traits
                    .iter()
                    .filter_map(|t| self.resolve_trait(t))
                    .collect()
            })
            .collect()
    }

    /// Checks that the methods of `impl Trait for Type` match the trait.
    pub(super) fn check_trait_impl(&mut self, decl: &ast::ImplDecl, trait_id: TraitId, def: Def) {
        let trait_name = self.traits[trait_id.0].name.clone();
        let type_name = decl.type_name.name.clone();
        if trait_id.0 <= LAST_BUILTIN.0 {
            self.diags.push(
                Diagnostic::error(format!("`{trait_name}` cannot be implemented by hand"))
                    .with_code("E0247")
                    .with_primary(decl.trait_name.as_ref().map_or(decl.span, |t| t.span), "")
                    .with_note(format!(
                        "every type whose values can all be {} implements `{trait_name}`",
                        match trait_id {
                            EQ => "compared",
                            SHOW => "shown",
                            ORD => "ordered with `<`",
                            KEY => "used as map keys",
                            _ => "turned into JSON",
                        }
                    )),
            );
            return;
        }
        if !self.trait_impls.insert((trait_id, def)) {
            self.diags.push(
                Diagnostic::error(format!("`{type_name}` already implements `{trait_name}`"))
                    .with_code("E0247")
                    .with_primary(decl.type_name.span, ""),
            );
            return;
        }
        let self_ty = self.own_type(def);
        let methods = self.traits[trait_id.0].methods.clone();
        for method in &decl.methods {
            if !methods.iter().any(|m| m.name == method.name.name) {
                self.diags.push(
                    Diagnostic::error(format!(
                        "method `{}` is not part of trait `{trait_name}`",
                        method.name.name
                    ))
                    .with_code("E0247")
                    .with_primary(method.name.span, "")
                    .with_help(format!("move it to a separate `impl {type_name}` block")),
                );
            }
        }
        for wanted in &methods {
            let Some(found) = decl.methods.iter().find(|m| m.name.name == wanted.name) else {
                self.diags.push(
                    Diagnostic::error(format!(
                        "`{type_name}` does not implement method `{}` of trait `{trait_name}`",
                        wanted.name
                    ))
                    .with_code("E0247")
                    .with_primary(decl.type_name.span, format!("`{}` is missing", wanted.name))
                    .with_secondary(wanted.span, "declared in the trait here"),
                );
                continue;
            };
            let Some(&id) = self.methods.get(&(def, wanted.name.clone())) else {
                continue;
            };
            let expected_params: Vec<Type> = wanted
                .params
                .iter()
                .map(|t| self.types.subst(*t, &[self_ty]))
                .collect();
            let expected_ret = self.types.subst(wanted.ret, &[self_ty]);
            let sig = &self.sigs[id.0];
            let found_params: Vec<Type> = sig.params.iter().skip(1).map(|p| p.ty).collect();
            let receiver_ok = sig.receiver == Some(false);
            if !receiver_ok || found_params != expected_params || sig.ret != expected_ret {
                let shown = |c: &Self, params: &[Type], ret: Type| {
                    let params: Vec<String> = params.iter().map(|t| c.show(*t)).collect();
                    let ret = match ret {
                        Type::Void => String::new(),
                        ret => format!(" -> {}", c.show(ret)),
                    };
                    let comma = if params.is_empty() { "" } else { ", " };
                    format!("fn {}(self{comma}{}){ret}", wanted.name, params.join(", "))
                };
                let expected = shown(self, &expected_params, expected_ret);
                self.diags.push(
                    Diagnostic::error(format!(
                        "method `{}` does not match trait `{trait_name}`",
                        wanted.name
                    ))
                    .with_code("E0247")
                    .with_primary(found.name.span, "")
                    .with_secondary(wanted.span, "declared in the trait here")
                    .with_note(format!("the trait expects `{expected}`")),
                );
            }
        }
    }

    /// Whether `ty` implements `trait_id`, in the current function (whose
    /// type parameters have bounds).
    pub(super) fn implements(&mut self, ty: Type, trait_id: TraitId) -> bool {
        if ty == Type::Error {
            return true;
        }
        if trait_id.0 <= LAST_BUILTIN.0 {
            return self.not_comparable(ty, trait_id).is_none();
        }
        match ty {
            Type::Param(i) => self
                .type_bounds
                .get(i as usize)
                .is_some_and(|b| b.contains(&trait_id)),
            Type::Named(id) => {
                let def = self.types.get(id).def;
                self.trait_impls.contains(&(trait_id, def))
            }
            _ => false,
        }
    }

    /// Reports type arguments of a call that do not implement the bounds
    /// of `bounds`.
    pub(super) fn check_bounds(
        &mut self,
        name: &str,
        bounds: &[Vec<TraitId>],
        type_args: &[Type],
        span: Span,
    ) {
        for (bounds, ty) in bounds.iter().zip(type_args) {
            for &trait_id in bounds {
                if !self.implements(*ty, trait_id) {
                    let shown = self.show(*ty);
                    let trait_name = self.traits[trait_id.0].name.clone();
                    let mut diag =
                        Diagnostic::error(format!("`{shown}` does not implement `{trait_name}`"))
                            .with_code("E0248")
                            .with_primary(span, format!("`{name}` needs `{trait_name}`"));
                    diag = match ty {
                        Type::Param(_) => diag.with_help(format!(
                            "add the bound to this function's type parameter: `{shown}: {trait_name}`"
                        )),
                        _ if trait_id.0 <= LAST_BUILTIN.0 => diag.with_note(format!(
                            "`{trait_name}` needs every value inside `{shown}` to support it"
                        )),
                        Type::Named(id)
                            if matches!(
                                self.types.get(*id).def,
                                Def::Struct(_) | Def::Enum(_)
                            ) && self.def_span(self.types.get(*id).def).is_some() =>
                        {
                            let def = self.types.get(*id).def;
                            let name = self.def_name(def).to_string();
                            diag.with_help(format!("write `impl {trait_name} for {name}`"))
                        }
                        _ => diag.with_note("only declared structs and enums can implement traits"),
                    };
                    self.diags.push(diag);
                }
            }
        }
    }

    /// `value.method(args)` where `value` has a type parameter's type: a
    /// method of one of the parameter's bounds.
    pub(super) fn trait_method_call(
        &mut self,
        receiver: CheckedExpr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let Type::Param(i) = receiver.ty else {
            unreachable!("called for type parameters")
        };
        let bounds = self
            .type_bounds
            .get(i as usize)
            .cloned()
            .unwrap_or_default();
        let found = bounds.iter().find_map(|t| {
            self.traits[t.0]
                .methods
                .iter()
                .find(|m| m.name == name.name)
                .cloned()
        });
        let Some(method) = found else {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let ty = self.show(receiver.ty);
            let mut diag = Diagnostic::error(format!("`{ty}` has no method `{}`", name.name))
                .with_code("E0231")
                .with_primary(name.span, "unknown method");
            diag = if bounds.is_empty() {
                diag.with_note(format!(
                    "`{ty}` can be any type; a bound such as `{ty}: SomeTrait` gives it methods"
                ))
            } else {
                let names: Vec<String> = bounds
                    .iter()
                    .map(|t| format!("`{}`", self.traits[t.0].name))
                    .collect();
                diag.with_note(format!(
                    "the methods of `{ty}` come from its bounds ({}), which do not declare `{}`",
                    names.join(", "),
                    name.name
                ))
            };
            self.diags.push(diag);
            return error_expr(span);
        };
        let params: Vec<Type> = method
            .params
            .iter()
            .map(|t| self.types.subst(*t, &[receiver.ty]))
            .collect();
        let ret = self.types.subst(method.ret, &[receiver.ty]);
        let Some(args) = self.fixed_args(&name.name, &params, args, span, ctx) else {
            return error_expr(span);
        };
        CheckedExpr {
            kind: CheckedExprKind::TraitCall {
                method: name.name.clone(),
                receiver: Box::new(receiver),
                args,
            },
            ty: ret,
            span,
        }
    }
}

/// Traits by name, starting with the built-in ones.
pub(super) fn builtin_trait_names() -> HashMap<String, TraitId> {
    [
        ("Eq".to_string(), EQ),
        ("Show".to_string(), SHOW),
        ("Ord".to_string(), ORD),
        ("Key".to_string(), KEY),
        ("Data".to_string(), DATA),
    ]
    .into_iter()
    .collect()
}
