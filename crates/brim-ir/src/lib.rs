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
//! - Lists are reference-counted values with **copy-on-write**: a list is
//!   only copied when it is changed while shared. Instructions that change a
//!   list ([`InstKind::ListPush`] and friends) and [`InstKind::Insert`]
//!   **consume** their list or struct operand: the value may not be used
//!   afterwards, which lets backends update it in place when nothing else
//!   refers to it. [`InstKind::Take`] moves a value out of a slot for that
//!   purpose.
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
id_type!(
    /// Index into [`Module::lists`].
    ListId,
    "L"
);
id_type!(
    /// Index into [`Module::maps`].
    MapId,
    "M"
);

/// IR types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    I64,
    /// IEEE 754 double.
    F64,
    Bool,
    Str,
    Struct(StructId),
    /// A list; its element type is `Module::lists[id]`.
    List(ListId),
    /// A map; its key and value types are `Module::maps[id]`. Keys are
    /// `i64`, `bool` or `str`.
    Map(MapId),
    /// Only valid as a function return type.
    Void,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::I64 => "i64",
            Type::F64 => "f64",
            Type::Bool => "bool",
            Type::Str => "str",
            Type::Struct(id) => return write!(f, "{id}"),
            Type::List(id) => return write!(f, "{id}"),
            Type::Map(id) => return write!(f, "{id}"),
            Type::Void => "void",
        })
    }
}

impl Type {
    /// The IR type of an ABI type; `None` for `StrList`, which is any list
    /// type whose elements are `str`.
    pub fn from_abi(t: brim_runtime::AbiType) -> Option<Type> {
        Some(match t {
            brim_runtime::AbiType::I64 => Type::I64,
            brim_runtime::AbiType::F64 => Type::F64,
            brim_runtime::AbiType::Bool => Type::Bool,
            brim_runtime::AbiType::Str => Type::Str,
            brim_runtime::AbiType::Void => Type::Void,
            brim_runtime::AbiType::StrList | brim_runtime::AbiType::IntList => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub structs: Vec<StructDef>,
    /// The element type of each list type.
    pub lists: Vec<Type>,
    /// The key and value types of each map type.
    pub maps: Vec<(Type, Type)>,
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

    pub fn list_elem(&self, id: ListId) -> Type {
        self.lists[id.index()]
    }

    pub fn map_types(&self, id: MapId) -> (Type, Type) {
        self.maps[id.index()]
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
    /// The function owns its first parameter: callers pass it with
    /// [`InstKind::CallMut`], which moves the value instead of sharing it.
    pub consumes_self: bool,
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
    /// A call that moves its first argument into the callee, which must
    /// have `consumes_self` set. Used for methods that change `self`.
    CallMut(FuncId, Vec<ValueId>),
    /// A call into the runtime library.
    CallRuntime(RuntimeFn, Vec<ValueId>),
    /// A struct value from one value per field, in field order.
    MakeStruct(StructId, Vec<ValueId>),
    /// Field `index` of a struct value.
    Extract(ValueId, usize),
    /// The struct value with field `index` replaced. Consumes the struct.
    Insert(ValueId, usize, ValueId),
    /// Moves the value out of a slot, leaving it unset: the slot must be
    /// stored before it is loaded again.
    Take(SlotId),
    /// A list of the given elements.
    ListNew(ListId, Vec<ValueId>),
    /// The number of elements (`i64`).
    ListLen(ValueId),
    /// Element `index`. An index outside the list is a run-time error.
    ListGet(ValueId, ValueId),
    /// The list with element `index` replaced. Consumes the list.
    ListSet(ValueId, ValueId, ValueId),
    /// The list with an element appended. Consumes the list.
    ListPush(ValueId, ValueId),
    /// The list's first `n` elements (`0 <= n <= len`). Consumes the list.
    ListTruncate(ValueId, ValueId),
    /// The list with an element inserted before `index` (`0 <= index <=
    /// len`). Consumes the list.
    ListInsert(ValueId, ValueId, ValueId),
    /// The list without element `index`. Consumes the list.
    ListRemove(ValueId, ValueId),
    /// Whether the list has an element equal to the value. Only for lists
    /// of `i64`, `bool` and `str`.
    ListContains(ValueId, ValueId),
    /// The elements of a list of `str`, joined with a separator.
    ListJoin(ValueId, ValueId),
    /// An empty map.
    MapNew(MapId),
    /// The number of entries (`i64`).
    MapLen(ValueId),
    /// The position of a key (`i64`), or -1 if the map does not have it.
    /// Entries are kept in insertion order, at positions `0..len`.
    MapFind(ValueId, ValueId),
    /// The position of a key; a missing key is a run-time error.
    MapIndex(ValueId, ValueId),
    /// The key at a position.
    MapKeyAt(ValueId, ValueId),
    /// The value at a position.
    MapValueAt(ValueId, ValueId),
    /// The map with a key set to a value (added at the end if new).
    /// Consumes the map.
    MapInsert(ValueId, ValueId, ValueId),
    /// The map with the value at a position replaced. Consumes the map.
    MapSetValueAt(ValueId, ValueId, ValueId),
    /// The map without the entry at a position. Consumes the map.
    MapRemoveAt(ValueId, ValueId),
}

impl InstKind {
    /// The operand this instruction consumes, if any.
    pub fn consumed(&self) -> Option<ValueId> {
        match self {
            InstKind::Insert(v, ..)
            | InstKind::ListSet(v, ..)
            | InstKind::ListPush(v, _)
            | InstKind::ListTruncate(v, _)
            | InstKind::ListInsert(v, ..)
            | InstKind::ListRemove(v, _)
            | InstKind::MapInsert(v, ..)
            | InstKind::MapSetValueAt(v, ..)
            | InstKind::MapRemoveAt(v, _) => Some(*v),
            InstKind::CallMut(_, args) => args.first().copied(),
            _ => None,
        }
    }

    /// Every value this instruction reads, in order.
    pub fn operands(&self) -> Vec<ValueId> {
        match self {
            InstKind::Const(_) | InstKind::Load(_) | InstKind::Take(_) | InstKind::MapNew(_) => {
                Vec::new()
            }
            InstKind::Store(_, v)
            | InstKind::Unary(_, v)
            | InstKind::Extract(v, _)
            | InstKind::ListLen(v)
            | InstKind::MapLen(v) => vec![*v],
            InstKind::Binary(_, a, b)
            | InstKind::Insert(a, _, b)
            | InstKind::ListGet(a, b)
            | InstKind::ListPush(a, b)
            | InstKind::ListTruncate(a, b)
            | InstKind::ListRemove(a, b)
            | InstKind::ListContains(a, b)
            | InstKind::ListJoin(a, b)
            | InstKind::MapFind(a, b)
            | InstKind::MapIndex(a, b)
            | InstKind::MapKeyAt(a, b)
            | InstKind::MapValueAt(a, b)
            | InstKind::MapRemoveAt(a, b) => vec![*a, *b],
            InstKind::ListSet(a, b, c)
            | InstKind::ListInsert(a, b, c)
            | InstKind::MapInsert(a, b, c)
            | InstKind::MapSetValueAt(a, b, c) => vec![*a, *b, *c],
            InstKind::Call(_, args)
            | InstKind::CallMut(_, args)
            | InstKind::CallRuntime(_, args)
            | InstKind::MakeStruct(_, args)
            | InstKind::ListNew(_, args) => args.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Const {
    I64(i64),
    /// The bits of an `f64` (so constants compare and hash exactly).
    F64(u64),
    Bool(bool),
    Str(String),
}

impl Const {
    pub fn ty(&self) -> Type {
        match self {
            Const::I64(_) => Type::I64,
            Const::F64(_) => Type::F64,
            Const::Bool(_) => Type::Bool,
            Const::Str(_) => Type::Str,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// Checked `i64` negation.
    INeg,
    /// `f64` negation.
    FNeg,
    /// Boolean not.
    BNot,
    /// The `i64` operand unchanged, if `min <= operand <= max`; otherwise
    /// an integer overflow in `op`. Keeps `i32` and `u8` values in range.
    Fit {
        min: i64,
        max: i64,
        op: &'static str,
    },
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
    /// `u64` operations on the bits of an `i64`: arithmetic traps when the
    /// result is outside `0..2^64`, comparisons are unsigned.
    UAdd,
    USub,
    UMul,
    UDiv,
    URem,
    ULt,
    ULe,
    UGt,
    UGe,
    /// IEEE 754 arithmetic on `f64`: no run-time errors (`1.0 / 0.0` is
    /// `inf`). `FRem` is the remainder with the sign of the dividend.
    FAdd,
    FSub,
    FMul,
    FDiv,
    FRem,
    /// Comparisons; any comparison with NaN is false, except `FNe`.
    FEq,
    FNe,
    FLt,
    FLe,
    FGt,
    FGe,
}

impl UnaryOp {
    /// `(operand type, result type)`.
    pub fn signature(self) -> (Type, Type) {
        match self {
            UnaryOp::INeg | UnaryOp::Fit { .. } => (Type::I64, Type::I64),
            UnaryOp::FNeg => (Type::F64, Type::F64),
            UnaryOp::BNot => (Type::Bool, Type::Bool),
        }
    }

    pub fn mnemonic(self) -> &'static str {
        match self {
            UnaryOp::INeg => "ineg",
            UnaryOp::FNeg => "fneg",
            UnaryOp::BNot => "bnot",
            UnaryOp::Fit { .. } => "fit",
        }
    }
}

impl BinaryOp {
    /// `(operand type, result type)`; both operands have the same type.
    pub fn signature(self) -> (Type, Type) {
        use BinaryOp::*;
        match self {
            IAdd | ISub | IMul | IDiv | IRem | UAdd | USub | UMul | UDiv | URem => {
                (Type::I64, Type::I64)
            }
            IEq | INe | ILt | ILe | IGt | IGe | ULt | ULe | UGt | UGe => (Type::I64, Type::Bool),
            BEq | BNe => (Type::Bool, Type::Bool),
            FAdd | FSub | FMul | FDiv | FRem => (Type::F64, Type::F64),
            FEq | FNe | FLt | FLe | FGt | FGe => (Type::F64, Type::Bool),
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
            UAdd => "uadd",
            USub => "usub",
            UMul => "umul",
            UDiv => "udiv",
            URem => "urem",
            ULt => "ult",
            ULe => "ule",
            UGt => "ugt",
            UGe => "uge",
            FAdd => "fadd",
            FSub => "fsub",
            FMul => "fmul",
            FDiv => "fdiv",
            FRem => "frem",
            FEq => "feq",
            FNe => "fne",
            FLt => "flt",
            FLe => "fle",
            FGt => "fgt",
            FGe => "fge",
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
