//! The editor index (`index_modules`): what names refer to, the types of
//! expressions and the members of types, for the language server.
//!
//! It is gathered while checking, so it is there also for programs with
//! errors. Only the program's own modules are indexed, not `std`.

use brim_ast as ast;
use brim_diagnostics::Span;

use super::FnId;
use super::{Checker, FnCtx};
use crate::checked::{CheckedExpr, CheckedExprKind, EditorIndex, Member, MemberKind};
use crate::{Def, EnumId, Type};

impl Checker<'_> {
    fn indexing(&self) -> bool {
        self.index.is_some() && !self.in_std_module()
    }

    /// Records that the name at `used` refers to the declaration at `decl`.
    pub(super) fn note_ref(&mut self, used: Span, decl: Span) {
        if self.indexing()
            && let Some(index) = &mut self.index
        {
            index.refs.push((used, decl));
        }
    }

    /// Records the type of the expression or variable at `span`.
    pub(super) fn note_type(&mut self, span: Span, ty: Type) {
        if ty != Type::Error
            && self.indexing()
            && let Some(index) = &mut self.index
        {
            index.types.push((span, ty));
        }
    }

    /// Records what an expression's names refer to, and its type.
    pub(super) fn note_expr(&mut self, expr: &ast::Expr, checked: &CheckedExpr, ctx: &FnCtx) {
        if !self.indexing() {
            return;
        }
        self.note_type(expr.span, checked.ty);
        match (&expr.kind, &checked.kind) {
            (ast::ExprKind::Name(ident), CheckedExprKind::Local(id)) => {
                let decl = local_span(ctx, id.0);
                self.note_ref(ident.span, decl);
            }
            (ast::ExprKind::Name(ident), CheckedExprKind::FnRef(id)) => {
                self.note_fn(ident.span, *id);
            }
            (
                ast::ExprKind::Call { callee, .. },
                CheckedExprKind::Call { func, .. } | CheckedExprKind::CallMut { func, .. },
            ) => self.note_fn(callee.span, *func),
            (
                ast::ExprKind::MethodCall { name, .. },
                CheckedExprKind::Call { func, .. } | CheckedExprKind::CallMut { func, .. },
            ) => self.note_fn(name.span, *func),
            (ast::ExprKind::Field { base, field }, CheckedExprKind::Field { index, .. }) => {
                if let Some(Def::Struct(s)) = self.def_of(self.expr_type(base.span)) {
                    let decl = self.structs[s.0].fields[*index].name_span;
                    self.note_ref(field.span, decl);
                }
            }
            (
                ast::ExprKind::Name(name)
                | ast::ExprKind::Field { field: name, .. }
                | ast::ExprKind::MethodCall { name, .. },
                CheckedExprKind::Variant { index, .. },
            ) => {
                if let Some(Def::Enum(e)) = self.def_of(checked.ty)
                    && let Some(decl) = self.enums[e.0].variants[*index].name_span
                {
                    self.note_ref(name.span, decl);
                }
            }
            (
                ast::ExprKind::StructLit { name, fields },
                CheckedExprKind::StructLit {
                    fields: checked_fields,
                },
            ) => {
                if let Some(Def::Struct(s)) = self.def_of(checked.ty) {
                    let decl = self.structs[s.0].name_span;
                    self.note_ref(name.span, decl);
                    for (init, (index, _)) in fields.iter().zip(checked_fields) {
                        let decl = self.structs[s.0].fields[*index].name_span;
                        self.note_ref(init.name.span, decl);
                    }
                }
            }
            _ => {}
        }
    }

    fn note_fn(&mut self, used: Span, id: FnId) {
        let decl = self.sigs[id.0].name_span;
        self.note_ref(used, decl);
    }

    /// The type recorded for the expression at `span`.
    fn expr_type(&self, span: Span) -> Type {
        self.index
            .as_ref()
            .and_then(|i| i.types.iter().rev().find(|(s, _)| *s == span))
            .map_or(Type::Error, |(_, t)| *t)
    }

    fn def_of(&self, ty: Type) -> Option<Def> {
        match ty {
            Type::Named(id) => Some(self.types.get(id).def),
            _ => None,
        }
    }

    /// Completes the index: the text and the members of every type in it,
    /// and the members of every declared type.
    pub(super) fn finish_index(&mut self) -> Option<EditorIndex> {
        let mut index = self.index.take()?;
        let mut seen: Vec<Type> = index.types.iter().map(|(_, t)| *t).collect();
        seen.sort_by_key(|t| format!("{t:?}"));
        seen.dedup();
        for ty in seen {
            index.type_text.insert(ty, self.show(ty));
            let members = self.members_of(ty);
            index.members.insert(ty, members);
        }
        for s in 0..self.structs.len() {
            let def = Def::Struct(crate::StructId(s));
            let members = self.static_members(def);
            index
                .type_members
                .insert(self.structs[s].name.clone(), members);
        }
        for e in 0..self.enums.len() {
            let def = Def::Enum(EnumId(e));
            let variants: Vec<(String, Vec<Type>)> = self.enums[e]
                .variants
                .iter()
                .map(|v| (v.name.clone(), v.fields.clone()))
                .collect();
            let mut members: Vec<Member> = Vec::new();
            for (name, fields) in variants {
                let detail = if fields.is_empty() {
                    name.clone()
                } else {
                    let shown: Vec<String> =
                        self.with_params(def, |c| fields.iter().map(|t| c.show(*t)).collect());
                    format!("{name}({})", shown.join(", "))
                };
                members.push(Member {
                    name,
                    kind: MemberKind::Variant,
                    detail,
                });
            }
            members.extend(self.static_members(def));
            index
                .type_members
                .insert(self.enums[e].name.clone(), members);
        }
        Some(index)
    }

    /// Runs `f` with the type parameters of `def` as the names of
    /// `Type::Param`s, for showing its fields.
    fn with_params<T>(&mut self, def: Def, f: impl FnOnce(&mut Self) -> T) -> T {
        let params = match def {
            Def::Struct(id) => self.structs[id.0].type_params.clone(),
            Def::Enum(id) => self.enums[id.0].type_params.clone(),
            _ => Vec::new(),
        };
        let saved = std::mem::replace(&mut self.type_params, params);
        let out = f(self);
        self.type_params = saved;
        out
    }

    /// What `value.` can be followed by for a value of type `ty`.
    fn members_of(&mut self, ty: Type) -> Vec<Member> {
        let builtin = |names: &[&str], kind: &str| -> Vec<Member> {
            names
                .iter()
                .map(|n| Member {
                    name: (*n).to_string(),
                    kind: MemberKind::Method,
                    detail: format!("{kind} method"),
                })
                .collect()
        };
        let (def, args) = match ty {
            Type::String => {
                let mut out = builtin(super::strings::STR_METHODS, "string");
                out.extend(self.prelude_members("string"));
                return dedup(out);
            }
            Type::Named(id) => {
                let named = self.types.get(id).clone();
                (named.def, named.args)
            }
            _ => return Vec::new(),
        };
        let mut out = match def {
            Def::List => {
                let mut out = builtin(super::collections::LIST_METHODS, "list");
                out.extend(self.prelude_members("list"));
                out
            }
            Def::Map => builtin(super::collections::MAP_METHODS, "map"),
            Def::Set => {
                let mut out = builtin(super::collections::SET_METHODS, "set");
                out.extend(self.prelude_members("set"));
                out
            }
            Def::Box => builtin(&["value"], "box"),
            Def::Tuple => (0..args.len())
                .map(|i| Member {
                    name: i.to_string(),
                    kind: MemberKind::Field,
                    detail: format!("{i}: {}", self.show(args[i])),
                })
                .collect(),
            Def::Enum(EnumId(0)) => self.prelude_members("option"),
            Def::Enum(EnumId(1)) => self.prelude_members("result"),
            Def::Struct(s) => {
                let fields: Vec<(String, Type)> = self.structs[s.0]
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), self.types.subst(f.ty, &args)))
                    .collect();
                fields
                    .into_iter()
                    .map(|(name, t)| Member {
                        detail: format!("{name}: {}", self.show(t)),
                        name,
                        kind: MemberKind::Field,
                    })
                    .collect()
            }
            Def::Enum(_) | Def::Fn => Vec::new(),
        };
        out.extend(self.methods_of(def, true));
        dedup(out)
    }

    /// The prelude's `<kind>_<name>` functions, which are methods of
    /// strings, lists, sets, `Option` and `Result`.
    fn prelude_members(&mut self, kind: &str) -> Vec<Member> {
        let Some(module) = self.module_names.iter().position(|n| n == "std.prelude") else {
            return Vec::new();
        };
        let prefix = format!("{kind}_");
        let mut found: Vec<(String, FnId)> = self.scopes[module]
            .own_fns
            .iter()
            .filter_map(|(name, id)| name.strip_prefix(&prefix).map(|n| (n.to_string(), *id)))
            .filter(|(name, _)| !name.starts_with("sum_"))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<Member> = found
            .into_iter()
            .map(|(name, id)| Member {
                detail: self.member_signature(id, &name, true),
                name,
                kind: MemberKind::Method,
            })
            .collect();
        if kind == "list" {
            out.push(Member {
                name: "sum".into(),
                kind: MemberKind::Method,
                detail: "fn sum() -> T".into(),
            });
        }
        out
    }

    /// The methods declared for `def`: with `self` when `with_self`, else
    /// the functions called on the type itself.
    fn methods_of(&mut self, def: Def, with_self: bool) -> Vec<Member> {
        let mut found: Vec<(String, FnId)> = self
            .methods
            .iter()
            .filter(|((d, _), _)| *d == def)
            .map(|((_, name), id)| (name.clone(), *id))
            .filter(|(_, id)| self.sigs[id.0].receiver.is_some() == with_self)
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
            .into_iter()
            .map(|(name, id)| Member {
                detail: self.member_signature(id, &name, with_self),
                name,
                kind: MemberKind::Method,
            })
            .collect()
    }

    fn static_members(&mut self, def: Def) -> Vec<Member> {
        self.methods_of(def, false)
    }

    /// `fn name(params) -> ret`, without the receiver when `skip_self`.
    fn member_signature(&mut self, id: FnId, name: &str, skip_self: bool) -> String {
        let sig = &self.sigs[id.0];
        let names = sig.type_params.clone();
        let skip = usize::from(skip_self && (sig.receiver.is_some() || !sig.method));
        let params: Vec<(String, Type)> = sig
            .params
            .iter()
            .skip(skip)
            .map(|p| (p.name.clone(), p.ty))
            .collect();
        let ret = sig.ret;
        let saved = std::mem::replace(&mut self.type_params, names);
        let params: Vec<String> = params
            .iter()
            .map(|(n, t)| format!("{n}: {}", self.show(*t)))
            .collect();
        let ret = match ret {
            Type::Void => String::new(),
            t => format!(" -> {}", self.show(t)),
        };
        self.type_params = saved;
        format!("fn {name}({}){ret}", params.join(", "))
    }
}

fn local_span(ctx: &FnCtx, id: usize) -> Span {
    ctx.locals[id].span
}

fn dedup(mut members: Vec<Member>) -> Vec<Member> {
    let mut seen = std::collections::HashSet::new();
    members.retain(|m| seen.insert(m.name.clone()));
    members
}
