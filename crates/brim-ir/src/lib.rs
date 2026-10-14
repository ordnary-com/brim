//! Brim IR: the compiler's intermediate representation.
//!
//! The IR sits between the checked program and every backend (the reference
//! interpreter today, LLVM later). It is deliberately independent of both the
//! AST and LLVM:
//!
//! - A [`Module`] is a list of [`Function`]s plus an entry point.
//! - A function body is a control-flow graph of [`Block`]s. Each block is a
//!   list of [`Inst`]ructions followed by exactly one [`Terminator`].
//! - Named variables live in **slots** (typed, mutable storage, like LLVM
//!   `alloca`). Parameters are the first `param_count` slots, initialised by
//!   the caller.
//! - Intermediate results are **values**: each [`ValueId`] is assigned by
//!   exactly one instruction and may only be used later in the *same* block.
//!   Data flows between blocks only through slots. This keeps the IR easy to
//!   validate and interpret, and maps directly onto LLVM's `alloca` +
//!   `mem2reg` approach.
//! - Structs are **aggregate values**, like LLVM's first-class structs:
//!   [`InstKind::MakeStruct`] builds one, [`InstKind::Extract`] reads a field
//!   and [`InstKind::Insert`] returns a copy with one field replaced. A struct
//!   never contains itself, so every struct has a finite, fixed layout.
//! - Operations are fully typed (`iadd` vs. `call @brim_rt_str_concat`); there
//!   is no overloading or implicit conversion left at this level.
//! - Instructions may carry a source [`Span`] used for run-time error
//!   messages (and, later, debug information).
//!
//! [`validate`] checks all of these structural and typing rules. The textual
//! form produced by `Display` is documented in `docs/architecture.md`.

mod display;
mod validate;

use std::fmt;

pub use brim_diagnostics::Span;
pub use brim_runtime::RuntimeFn;
pub use validate::{ValidationError, validate};

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

id_type!(
    /// Index into [`Module::functions`].
    FuncId,
    "fn"
);
id_type!(
    /// Index into [`Function::blocks`]. Block 0 is the entry block.
    BlockId,
    "bb"
);
id_type!(
    /// Index into [`Function::slots`].
    SlotId,
    "s"
);
id_type!(
    /// Index into [`Function::values`].
    ValueId,
    "v"
);
id_type!(
    /// Index into [`Module::structs`].
    StructId,
    "%"
);

/// IR types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    Bool,
    Str,
    Struct(StructId),
    /// Only valid as a function return type.
    Void,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::I64 => "i64",
            Type::Bool => "bool",
            Type::Str => "str",
            Type::Struct(id) => return write!(f, "{id}"),
            Type::Void => "void",
        })
    }
}

impl From<brim_runtime::AbiType> for Type {
    fn from(t: brim_runtime::AbiType) -> Type {
        match t {
            brim_runtime::AbiType::I64 => Type::I64,
            brim_runtime::AbiType::Bool => Type::Bool,
            brim_runtime::AbiType::Str => Type::Str,
            brim_runtime::AbiType::Void => Type::Void,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub structs: Vec<StructDef>,
    pub functions: Vec<Function>,
    /// The function the program starts in. Takes no parameters, returns `void`.
    pub entry: FuncId,
}

impl Module {
    pub fn function(&self, id: FuncId) -> &Function {
        &self.functions[id.index()]
    }

    pub fn struct_def(&self, id: StructId) -> &StructDef {
        &self.structs[id.index()]
    }
}

/// A struct type: its fields, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDef {
    /// The source-level name, for readability and debug information.
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    /// The first `param_count` slots are the parameters.
    pub param_count: usize,
    pub ret: Type,
    pub slots: Vec<Slot>,
    /// The type of every value, indexed by [`ValueId`].
    pub values: Vec<Type>,
    pub blocks: Vec<Block>,
}

impl Function {
    pub fn param_types(&self) -> impl Iterator<Item = Type> + '_ {
        self.slots[..self.param_count.min(self.slots.len())]
            .iter()
            .map(|s| s.ty)
    }
}

/// Typed local storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The source-level name, for readability and debug information.
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Terminator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inst {
    /// The value defined by this instruction, if it produces one.
    pub result: Option<ValueId>,
    pub kind: InstKind,
    /// The source location responsible for this instruction.
    pub span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstKind {
    Const(Const),
    Load(SlotId),
    Store(SlotId, ValueId),
    Unary(UnaryOp, ValueId),
    Binary(BinaryOp, ValueId, ValueId),
    /// A call to a function in the same module.
    Call(FuncId, Vec<ValueId>),
    /// A call into the runtime library.
    CallRuntime(RuntimeFn, Vec<ValueId>),
    /// A struct value from one value per field, in field order.
    MakeStruct(StructId, Vec<ValueId>),
    /// Field `index` of a struct value.
    Extract(ValueId, usize),
    /// A copy of a struct value with field `index` replaced.
    Insert(ValueId, usize, ValueId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Const {
    I64(i64),
    Bool(bool),
    Str(String),
}

impl Const {
    pub fn ty(&self) -> Type {
        match self {
            Const::I64(_) => Type::I64,
            Const::Bool(_) => Type::Bool,
            Const::Str(_) => Type::Str,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// Checked `i64` negation.
    INeg,
    /// Boolean not.
    BNot,
}

/// Binary operations. Integer arithmetic is checked: overflow and division
/// by zero are run-time errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    IAdd,
    ISub,
    IMul,
    IDiv,
    IRem,
    IEq,
    INe,
    ILt,
    ILe,
    IGt,
    IGe,
    BEq,
    BNe,
}

impl UnaryOp {
    /// `(operand type, result type)`.
    pub fn signature(self) -> (Type, Type) {
        match self {
            UnaryOp::INeg => (Type::I64, Type::I64),
            UnaryOp::BNot => (Type::Bool, Type::Bool),
        }
    }

    pub fn mnemonic(self) -> &'static str {
        match self {
            UnaryOp::INeg => "ineg",
            UnaryOp::BNot => "bnot",
        }
    }
}

impl BinaryOp {
    /// `(operand type, result type)`; both operands have the same type.
    pub fn signature(self) -> (Type, Type) {
        use BinaryOp::*;
        match self {
            IAdd | ISub | IMul | IDiv | IRem => (Type::I64, Type::I64),
            IEq | INe | ILt | ILe | IGt | IGe => (Type::I64, Type::Bool),
            BEq | BNe => (Type::Bool, Type::Bool),
        }
    }

    pub fn mnemonic(self) -> &'static str {
        use BinaryOp::*;
        match self {
            IAdd => "iadd",
            ISub => "isub",
            IMul => "imul",
            IDiv => "idiv",
            IRem => "irem",
            IEq => "ieq",
            INe => "ine",
            ILt => "ilt",
            ILe => "ile",
            IGt => "igt",
            IGe => "ige",
            BEq => "beq",
            BNe => "bne",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Terminator {
    /// Return from the function, with a value unless it returns `void`.
    Return(Option<ValueId>),
    Jump(BlockId),
    Branch {
        cond: ValueId,
        then_block: BlockId,
        else_block: BlockId,
    },
    /// Control never reaches the end of this block.
    Unreachable,
}

#[cfg(test)]
mod tests;
