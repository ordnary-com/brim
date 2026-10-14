//! Semantic analysis for Brim: name resolution and type checking.
//!
//! [`check`] walks the AST produced by `brim-parser` and either produces a
//! [`CheckedProgram`] (a typed, name-resolved tree) or reports why the program
//! is invalid. It runs in two passes:
//!
//! 1. **Declaration collection**: every function signature is resolved and
//!    entered into the function table, so functions can be called before
//!    they are defined (and recursively).
//! 2. **Body checking**: each function body is checked with a stack of
//!    lexical scopes for local variables.
//!
//! The checker keeps going after an error to report as many independent
//! problems as possible. An internal [`Type::Error`] marks expressions whose
//! type could not be determined; operations involving it are not reported
//! again, which prevents cascades of follow-up errors.

pub mod checked;
mod checker;
mod suggest;

use std::fmt;

use brim_ast::Program;
use brim_diagnostics::Diagnostic;
use brim_stdlib::Prim;

pub use checked::CheckedProgram;

/// Index into [`CheckedProgram::structs`] (and the checker's struct table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub usize);

/// A Brim type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    Bool,
    String,
    /// A struct declared in the program. Its name and fields are in the
    /// struct table; `Display` can only show its index.
    Struct(StructId),
    /// The type of functions that return nothing. It has no values: it cannot
    /// be stored in a variable or passed as an argument.
    Void,
    /// The type of an expression that already produced an error.
    Error,
}

impl Type {
    /// Resolves a type name as written in source.
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
            Type::Struct(_) | Type::Error => return None,
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
            Type::Struct(id) => return write!(f, "struct#{}", id.0),
            Type::Error => "{error}",
        })
    }
}

/// Checks `program`.
///
/// Returns the checked program if there were no errors, together with all
/// diagnostics (warnings are returned in both cases).
pub fn check(program: &Program) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
    checker::Checker::check(program)
}

#[cfg(test)]
mod tests;
