//! The checked program: the output of semantic analysis.
//!
//! This is a typed, name-resolved tree. Every expression has a [`Type`],
//! every variable reference points at a [`LocalId`], every call points at a
//! [`FnId`] or a specific runtime function, and every operator has been
//! resolved to a type-specific operation. Later phases (lowering, future
//! tooling) work from this tree and never need to repeat name or type lookup.
//!
//! A `CheckedProgram` is only produced for programs without errors, so it
//! never contains [`Type::Error`].

use std::fmt;

use brim_diagnostics::Span;
use brim_runtime::RuntimeFn;

use crate::{StructId, Type};

/// Index into [`CheckedProgram::functions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FnId(pub usize);

/// Index into [`CheckedFn::locals`]. Parameters are the first locals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub usize);

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "local{}", self.0)
    }
}

#[derive(Clone, Debug)]
pub struct CheckedProgram {
    /// Indexed by [`StructId`]. Struct types never contain themselves,
    /// directly or indirectly.
    pub structs: Vec<CheckedStruct>,
    pub functions: Vec<CheckedFn>,
    /// The `main` function.
    pub entry: FnId,
}

#[derive(Clone, Debug)]
pub struct CheckedStruct {
    pub name: String,
    pub span: Span,
    /// In declaration order; field indices refer to this list.
    pub fields: Vec<CheckedField>,
}

#[derive(Clone, Debug)]
pub struct CheckedField {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct CheckedFn {
    pub name: String,
    pub span: Span,
    /// Number of parameters; parameters are `LocalId(0..param_count)`.
    pub param_count: usize,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub body: Vec<CheckedStmt>,
}

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: Type,
    /// Declared with `let mut`. Parameters are never mutable.
    pub mutable: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum CheckedStmt {
    Let {
        local: LocalId,
        init: CheckedExpr,
    },
    /// `local = value`, or with a non-empty `path` of field indices,
    /// `local.a.b = value`. Compound assignments are already expanded, so
    /// `x += 1` is `x = x + 1`.
    Assign {
        local: LocalId,
        path: Vec<usize>,
        value: CheckedExpr,
    },
    If {
        cond: CheckedExpr,
        then_body: Vec<CheckedStmt>,
        /// Empty when there is no `else`. An `else if` is a single nested
        /// `If` statement.
        else_body: Vec<CheckedStmt>,
    },
    While {
        cond: CheckedExpr,
        body: Vec<CheckedStmt>,
    },
    Break,
    Continue,
    Expr(CheckedExpr),
    Return {
        value: Option<CheckedExpr>,
        span: Span,
    },
    Block(Vec<CheckedStmt>),
}

#[derive(Clone, Debug)]
pub struct CheckedExpr {
    pub kind: CheckedExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum CheckedExprKind {
    Int(i64),
    Bool(bool),
    Str(String),
    Local(LocalId),
    Unary {
        op: UnaryOp,
        operand: Box<CheckedExpr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<CheckedExpr>,
        rhs: Box<CheckedExpr>,
    },
    /// Short-circuiting `&&`.
    And(Box<CheckedExpr>, Box<CheckedExpr>),
    /// Short-circuiting `||`.
    Or(Box<CheckedExpr>, Box<CheckedExpr>),
    Call {
        func: FnId,
        args: Vec<CheckedExpr>,
    },
    /// A call to a builtin, resolved to the runtime function implementing the
    /// selected overload.
    Builtin {
        runtime: RuntimeFn,
        args: Vec<CheckedExpr>,
    },
    /// A struct literal. `fields` holds every field exactly once, as
    /// `(field index, value)` in **source order**, which is the order the
    /// values are evaluated in.
    StructLit {
        id: StructId,
        fields: Vec<(usize, CheckedExpr)>,
    },
    /// `base.field`, with the field's index in its struct.
    Field {
        base: Box<CheckedExpr>,
        index: usize,
    },
}

/// Type-resolved unary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x` on `i64` (checked).
    IntNeg,
    /// `!x` on `bool`.
    BoolNot,
}

/// Type-resolved binary operators (excluding the short-circuit operators).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    IntAdd,
    IntSub,
    IntMul,
    IntDiv,
    IntRem,
    IntEq,
    IntNe,
    IntLt,
    IntLe,
    IntGt,
    IntGe,
    BoolEq,
    BoolNe,
    StrConcat,
    StrEq,
    StrNe,
}
