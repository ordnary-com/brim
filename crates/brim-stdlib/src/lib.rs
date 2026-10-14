//! The Brim standard library catalogue.
//!
//! For now the standard library consists only of *builtin functions*: names
//! that are always in scope and are implemented by the runtime rather than in
//! Brim source. This crate is the single place that declares them, so the type
//! checker, documentation and future tooling (editor completion, docs
//! generation) agree on what exists.
//!
//! A builtin may have several *overloads* that differ in parameter types; each
//! overload maps to exactly one [`RuntimeFn`]. User-defined functions cannot be
//! overloaded.
//!
//! When Brim gains modules, this catalogue is expected to become the `std`
//! prelude, with most of the library written in Brim itself.

use std::fmt;

use brim_runtime::RuntimeFn;

/// The primitive types the standard library is described in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    I64,
    Bool,
    String,
    Void,
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Prim::I64 => "i64",
            Prim::Bool => "bool",
            Prim::String => "string",
            Prim::Void => "void",
        })
    }
}

/// One signature of a builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overload {
    pub params: &'static [Prim],
    pub ret: Prim,
    /// The runtime function that implements this overload.
    pub runtime: RuntimeFn,
}

/// A builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    /// One-line description for documentation and tooling.
    pub summary: &'static str,
    pub overloads: &'static [Overload],
}

impl Builtin {
    /// Selects the overload whose parameter types exactly match `args`.
    pub fn resolve(&self, args: &[Prim]) -> Option<&'static Overload> {
        self.overloads.iter().find(|o| o.params == args)
    }

    /// The distinct numbers of parameters this builtin accepts.
    pub fn arities(&self) -> Vec<usize> {
        let mut arities: Vec<_> = self.overloads.iter().map(|o| o.params.len()).collect();
        arities.sort_unstable();
        arities.dedup();
        arities
    }
}

/// Every builtin function.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "print",
        summary: "Writes a value followed by a newline to standard output.",
        overloads: &[
            Overload {
                params: &[Prim::String],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintStr,
            },
            Overload {
                params: &[Prim::I64],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintI64,
            },
            Overload {
                params: &[Prim::Bool],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintBool,
            },
        ],
    },
    Builtin {
        name: "to_string",
        summary: "Converts a value to its textual representation.",
        overloads: &[
            Overload {
                params: &[Prim::I64],
                ret: Prim::String,
                runtime: RuntimeFn::I64ToStr,
            },
            Overload {
                params: &[Prim::Bool],
                ret: Prim::String,
                runtime: RuntimeFn::BoolToStr,
            },
        ],
    },
];

/// Looks up a builtin by name.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_runtime::AbiType;

    fn abi(p: Prim) -> AbiType {
        match p {
            Prim::I64 => AbiType::I64,
            Prim::Bool => AbiType::Bool,
            Prim::String => AbiType::Str,
            Prim::Void => AbiType::Void,
        }
    }

    #[test]
    fn print_resolves_by_argument_type() {
        let print = lookup("print").unwrap();
        assert_eq!(
            print.resolve(&[Prim::I64]).unwrap().runtime,
            RuntimeFn::PrintI64
        );
        assert_eq!(
            print.resolve(&[Prim::String]).unwrap().runtime,
            RuntimeFn::PrintStr
        );
        assert!(print.resolve(&[Prim::Void]).is_none());
        assert!(print.resolve(&[]).is_none());
        assert_eq!(print.arities(), vec![1]);
    }

    #[test]
    fn overloads_match_runtime_signatures() {
        for builtin in BUILTINS {
            for overload in builtin.overloads {
                let params: Vec<_> = overload.params.iter().map(|p| abi(*p)).collect();
                assert_eq!(params, overload.runtime.params(), "{}", builtin.name);
                assert_eq!(
                    abi(overload.ret),
                    overload.runtime.ret(),
                    "{}",
                    builtin.name
                );
            }
        }
    }

    #[test]
    fn to_string_resolves() {
        let to_string = lookup("to_string").unwrap();
        assert_eq!(to_string.resolve(&[Prim::I64]).unwrap().ret, Prim::String);
        assert!(to_string.resolve(&[Prim::String]).is_none());
    }

    #[test]
    fn unknown_names() {
        assert!(lookup("println").is_none());
    }
}
