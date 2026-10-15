//! Which types can be compared with `==` and turned into text with
//! `print` and `to_string`.
//!
//! Structs, enums, lists and maps can, when every value inside them can:
//! integers, floats, booleans and strings, or other such types. A type
//! parameter cannot, since generic code does not know what it stands for.

use std::collections::HashSet;

use super::Checker;
use crate::{Def, Type};

impl<'a> Checker<'a> {
    /// The first type inside `ty` (or `ty` itself) whose values cannot be
    /// compared or printed; `None` when all of them can.
    pub(super) fn not_comparable(&mut self, ty: Type) -> Option<Type> {
        self.not_comparable_inner(ty, &mut HashSet::new())
    }

    fn not_comparable_inner(&mut self, ty: Type, seen: &mut HashSet<Type>) -> Option<Type> {
        let Type::Named(id) = ty else {
            return match ty {
                Type::I64 | Type::F64 | Type::Bool | Type::String | Type::Error => None,
                _ => Some(ty),
            };
        };
        // A type that contains itself is fine if the rest of it is.
        if !seen.insert(ty) {
            return None;
        }
        let named = self.types.get(id).clone();
        let inner: Vec<Type> = match named.def {
            Def::List | Def::Map => named.args.clone(),
            Def::Struct(s) => self.structs[s.0].fields.iter().map(|f| f.ty).collect(),
            Def::Enum(e) => self.enums[e.0]
                .variants
                .iter()
                .flat_map(|v| v.fields.iter().copied())
                .collect(),
        };
        for field in inner {
            let field = self.types.subst(field, &named.args);
            if let Some(bad) = self.not_comparable_inner(field, seen) {
                return Some(bad);
            }
        }
        None
    }
}
