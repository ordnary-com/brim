//! Named types: structs and enums together with their type arguments.
//!
//! Every distinct `Name[args]` is stored once in a [`TypeTable`] and referred
//! to by its [`TypeId`], so [`Type`] stays a small `Copy` value and equal
//! types compare equal.

use std::collections::HashMap;

use crate::{EnumId, StructId, Type, TypeId};

/// The declaration a named type refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Def {
    Struct(StructId),
    Enum(EnumId),
    /// The built-in `List[T]`.
    List,
    /// The built-in `Map[K, V]`.
    Map,
    /// The built-in `Box[T]`: one immutable value on the heap, so that a
    /// type can contain itself.
    Box,
    /// A function type `fn(A, B) -> R`: the arguments are the parameter
    /// types followed by the return type (possibly `void`).
    Fn,
    /// A tuple type `(A, B, ...)`: the arguments are its element types.
    Tuple,
}

/// A struct or enum applied to type arguments (none for non-generic ones).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NamedType {
    pub def: Def,
    pub args: Vec<Type>,
}

#[derive(Clone, Debug, Default)]
pub struct TypeTable {
    named: Vec<NamedType>,
    index: HashMap<NamedType, TypeId>,
}

impl TypeTable {
    /// The type `def[args]`.
    pub fn intern(&mut self, def: Def, args: Vec<Type>) -> Type {
        let named = NamedType { def, args };
        if let Some(id) = self.index.get(&named) {
            return Type::Named(*id);
        }
        let id = TypeId(u32::try_from(self.named.len()).expect("type count fits in u32"));
        self.named.push(named.clone());
        self.index.insert(named, id);
        Type::Named(id)
    }

    pub fn get(&self, id: TypeId) -> &NamedType {
        &self.named[id.0 as usize]
    }

    /// Replaces every `Param(i)` in `ty` with `args[i]`.
    pub fn subst(&mut self, ty: Type, args: &[Type]) -> Type {
        match ty {
            Type::Param(i) => args[i as usize],
            Type::Named(id) => {
                let named = self.get(id).clone();
                if named.args.is_empty() {
                    return ty;
                }
                let new_args = named.args.iter().map(|t| self.subst(*t, args)).collect();
                self.intern(named.def, new_args)
            }
            other => other,
        }
    }

    /// Like [`subst`](Self::subst), but with possibly unknown arguments:
    /// `None` if `ty` uses one of them.
    pub fn subst_partial(&mut self, ty: Type, args: &[Option<Type>]) -> Option<Type> {
        match ty {
            Type::Param(i) => args[i as usize],
            Type::Named(id) => {
                let named = self.get(id).clone();
                if named.args.is_empty() {
                    return Some(ty);
                }
                let new_args = named
                    .args
                    .iter()
                    .map(|t| self.subst_partial(*t, args))
                    .collect::<Option<Vec<_>>>()?;
                Some(self.intern(named.def, new_args))
            }
            other => Some(other),
        }
    }

    /// Matches `pattern` (which may use type parameters) against `actual`,
    /// recording what each still-unknown parameter must be.
    pub fn unify(&self, pattern: Type, actual: Type, bindings: &mut [Option<Type>]) {
        match (pattern, actual) {
            (_, Type::Error) => {}
            (Type::Param(i), actual) => {
                let slot = &mut bindings[i as usize];
                if slot.is_none() {
                    *slot = Some(actual);
                }
            }
            (Type::Named(p), Type::Named(a)) => {
                let (p, a) = (self.get(p), self.get(a));
                if p.def == a.def {
                    for (pt, at) in p.args.iter().zip(&a.args) {
                        self.unify(*pt, *at, bindings);
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether `ty` mentions a type parameter.
    pub fn has_params(&self, ty: Type) -> bool {
        match ty {
            Type::Param(_) => true,
            Type::Named(id) => self.get(id).args.iter().any(|t| self.has_params(*t)),
            _ => false,
        }
    }

    /// Every struct and enum whose value is stored inline in a value of
    /// type `ty`, including through type arguments. Lists store their
    /// elements elsewhere, so they and their element types do not count.
    pub fn defs_in(&self, ty: Type, out: &mut Vec<Def>) {
        if let Type::Named(id) = ty {
            let named = self.get(id);
            if matches!(named.def, Def::List | Def::Map | Def::Box | Def::Fn) {
                return;
            }
            // A tuple holds its elements inline, but is no declaration.
            if named.def != Def::Tuple {
                out.push(named.def);
            }
            for arg in &named.args {
                self.defs_in(*arg, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_and_substitution() {
        let mut table = TypeTable::default();
        let option = |t: &mut TypeTable, arg| t.intern(Def::Enum(EnumId(0)), vec![arg]);
        let a = option(&mut table, Type::I64);
        let b = option(&mut table, Type::I64);
        assert_eq!(a, b);
        let generic = option(&mut table, Type::Param(0));
        assert!(table.has_params(generic));
        assert_eq!(table.subst(generic, &[Type::I64]), a);
        assert_eq!(table.subst_partial(generic, &[None]), None);

        let mut bindings = [None];
        table.unify(generic, a, &mut bindings);
        assert_eq!(bindings, [Some(Type::I64)]);
    }
}
