//! Semantic analysis for Brim: name resolution and type checking.
//!
//! [`check`] walks the AST produced by `brim-parser` and either produces a
//! [`CheckedProgram`] (a typed, name-resolved tree) or reports why the program
//! is invalid. It runs in two passes:
//!
//! 1. **Declaration collection**: every struct, enum and function signature
//!    is resolved and entered into the type and function tables, so they can
//!    be used before they are declared (and recursively).
//! 2. **Body checking**: each function body is checked with a stack of
//!    lexical scopes for local variables.
//!
//! Generic declarations are checked once, with their type parameters as
//! opaque types ([`Type::Param`]). Calls and literals infer type arguments
//! from their arguments and from the type the context expects. The checked
//! program stays generic; `brim-lower` instantiates it.
//!
//! The checker keeps going after an error to report as many independent
//! problems as possible. An internal [`Type::Error`] marks expressions whose
//! type could not be determined; operations involving it are not reported
//! again, which prevents cascades of follow-up errors.

pub mod checked;
mod checker;
mod suggest;
pub mod types;

use std::fmt;

use brim_ast::Program;
use brim_diagnostics::Diagnostic;
use brim_stdlib::Prim;

pub use checked::CheckedProgram;
pub use checker::ModuleAst;
pub use types::{Def, NamedType, TypeTable};

/// Index into [`CheckedProgram::structs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructId(pub usize);

/// Index into [`CheckedProgram::enums`]. The built-in enums `Option` and
/// `Result` come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumId(pub usize);

/// Index into a [`TypeTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// A Brim type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    Bool,
    String,
    /// The type of functions that return nothing. It has no values: it cannot
    /// be stored in a variable or passed as an argument.
    Void,
    /// A struct or enum, with its type arguments: an entry in the
    /// [`TypeTable`]. `Display` can only show its index.
    Named(TypeId),
    /// Type parameter number `n` of the generic declaration being checked.
    Param(u32),
    /// The type of an expression that already produced an error.
    Error,
}

impl Type {
    /// Resolves a built-in type name as written in source.
    pub fn from_name(name: &str) -> Option<Type> {
        Some(match name {
            "i64" => Type::I64,
            "bool" => Type::Bool,
            "string" => Type::String,
            "void" => Type::Void,
            _ => return None,
        })
    }

    pub fn to_prim(self) -> Option<Prim> {
        Some(match self {
            Type::I64 => Prim::I64,
            Type::Bool => Prim::Bool,
            Type::String => Prim::String,
            Type::Void => Prim::Void,
            Type::Named(_) | Type::Param(_) | Type::Error => return None,
        })
    }

    pub fn from_prim(prim: Prim) -> Type {
        match prim {
            Prim::I64 => Type::I64,
            Prim::Bool => Type::Bool,
            Prim::String => Type::String,
            Prim::Void => Type::Void,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::I64 => "i64",
            Type::Bool => "bool",
            Type::String => "string",
            Type::Void => "void",
            Type::Named(id) => return write!(f, "type#{}", id.0),
            Type::Param(n) => return write!(f, "param#{n}"),
            Type::Error => "{error}",
        })
    }
}

/// Checks `program`.
///
/// Returns the checked program if there were no errors, together with all
/// diagnostics (warnings are returned in both cases).
pub fn check(program: &Program) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
    let modules = [ModuleAst {
        name: String::new(),
        program: program.clone(),
    }];
    checker::Checker::check(&modules)
}

/// Checks a program of several modules. The first module is the root, which
/// contains `main`; the others are named by their dotted paths (`net.http`).
pub fn check_modules(modules: &[ModuleAst]) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
    checker::Checker::check(modules)
}

#[cfg(test)]
mod tests;
