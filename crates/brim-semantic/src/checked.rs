//! The checked program: the output of semantic analysis.
//!
//! This is a typed, name-resolved tree. Every expression has a [`Type`],
//! every variable reference points at a [`LocalId`], every call points at a
//! [`FnId`] or a specific runtime function, and every operator has been
//! resolved to a type-specific operation. Later phases (lowering, future
//! tooling) work from this tree and never need to repeat name or type lookup.
//!
//! A `CheckedProgram` is only produced for programs without errors, so it
//! never contains [`Type::Error`]. It is still generic: types inside generic
//! functions, structs and enums may contain [`Type::Param`], and generic
//! calls record their type arguments. `brim-lower` instantiates them.

use std::fmt;

use brim_diagnostics::Span;
use brim_runtime::RuntimeFn;

use crate::{Def, EnumId, StructId, Type, TypeTable};

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
    /// Every named type used by the program (more can be added by
    /// substituting type arguments).
    pub types: TypeTable,
    /// Indexed by [`StructId`]. Structs and enums never contain themselves,
    /// directly or indirectly.
    pub structs: Vec<CheckedStruct>,
    /// Indexed by [`EnumId`]; the built-in `Option` and `Result` come first.
    pub enums: Vec<CheckedEnum>,
    pub functions: Vec<CheckedFn>,
    /// The `main` function. Only a test build can be without one.
    pub entry: Option<FnId>,
    /// The names of the modules, indexed like [`ModuleAst`](crate::ModuleAst)s:
    /// the root module (named `""`) first.
    pub modules: Vec<String>,
    /// The test functions (`fn test_name()` outside `impl` blocks), in
    /// declaration order; only collected for a test build.
    pub tests: Vec<FnId>,
    /// Methods by the struct or enum they belong to and their name; trait
    /// calls are resolved through this when lowering.
    pub methods: std::collections::HashMap<(Def, String), FnId>,
}

#[derive(Clone, Debug)]
pub struct CheckedStruct {
    pub name: String,
    pub span: Span,
    /// The declaring module, an index into [`CheckedProgram::modules`].
    pub module: usize,
    /// Declared with `pub`.
    pub public: bool,
    /// Names of the type parameters; field types use `Type::Param(i)`.
    pub type_params: Vec<String>,
    /// In declaration order; field indices refer to this list.
    pub fields: Vec<CheckedField>,
}

#[derive(Clone, Debug)]
pub struct CheckedField {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct CheckedEnum {
    pub name: String,
    /// The declaring module, an index into [`CheckedProgram::modules`];
    /// `None` for the built-in `Option` and `Result`.
    pub module: Option<usize>,
    /// Declared with `pub` (true for the built-in enums).
    pub public: bool,
    pub type_params: Vec<String>,
    pub variants: Vec<CheckedVariant>,
}

#[derive(Clone, Debug)]
pub struct CheckedVariant {
    pub name: String,
    /// Payload types, in order; empty for a variant without payload.
    pub fields: Vec<Type>,
}

impl CheckedProgram {
    pub fn enum_def(&self, id: EnumId) -> &CheckedEnum {
        &self.enums[id.0]
    }

    pub fn struct_def(&self, id: StructId) -> &CheckedStruct {
        &self.structs[id.0]
    }
}

#[derive(Clone, Debug)]
pub struct CheckedFn {
    /// `name`, or `Type.name` for a method.
    pub name: String,
    /// A method with `mut self`: `self` (local 0) is taken from the caller
    /// and given back changed, together with the result.
    pub mut_self: bool,
    pub span: Span,
    /// Names of the type parameters of a generic function; types in the
    /// signature, locals and body use `Type::Param(i)`.
    pub type_params: Vec<String>,
    /// Number of parameters; parameters are `LocalId(0..param_count)`.
    pub param_count: usize,
    pub ret: Type,
    pub locals: Vec<Local>,
    pub body: Vec<CheckedStmt>,
    /// For a `fn` expression: local 0 is its environment, counted in
    /// `param_count`, and these locals start with the captured values, in
    /// the order of [`CheckedExprKind::Closure::captures`].
    pub captures: Option<Vec<LocalId>>,
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
    /// `place = value`, or a compound assignment such as `place += value`
    /// (`op` is then the resolved operator). The place's indices are
    /// evaluated once, before `value`.
    Assign {
        place: CheckedPlace,
        op: Option<BinaryOp>,
        value: CheckedExpr,
    },
    /// `for var in iter { body }`.
    For {
        var: LocalId,
        iter: CheckedForIter,
        body: Vec<CheckedStmt>,
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
    /// `match`. The arms are exhaustive; the first matching arm runs.
    Match {
        scrutinee: CheckedExpr,
        arms: Vec<CheckedArm>,
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

/// A variable, or a part of it: `items[i].name` is the local `items` with
/// the steps `[Index(i), Field(name)]`.
#[derive(Clone, Debug)]
pub struct CheckedPlace {
    pub local: LocalId,
    pub steps: Vec<PlaceStep>,
    /// The type of the part the place refers to.
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub enum PlaceStep {
    Field(usize),
    Index(CheckedExpr),
}

#[derive(Clone, Debug)]
pub enum CheckedForIter {
    /// The elements of a list, which is evaluated once.
    Each(CheckedExpr),
    /// The integers from `start` up to, not including, `end`; both are
    /// evaluated once.
    Range(CheckedExpr, CheckedExpr),
}

/// The built-in operations on lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOp {
    Len,
    IsEmpty,
    /// `Option[T]`: the element at an index, or `None`.
    Get,
    First,
    Last,
    Contains,
    Join,
    // Operations that change the list (their receiver is a place):
    Push,
    /// `Option[T]`: removes and returns the last element.
    Pop,
    Insert,
    /// Removes and returns the element at an index.
    Remove,
    Clear,
}

impl ListOp {
    pub fn changes_list(self) -> bool {
        matches!(
            self,
            ListOp::Push | ListOp::Pop | ListOp::Insert | ListOp::Remove | ListOp::Clear
        )
    }
}

/// The built-in operations on maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapOp {
    Len,
    IsEmpty,
    /// `Option[V]`.
    Get,
    ContainsKey,
    /// `List[K]`, in insertion order.
    Keys,
    /// `List[V]`, in insertion order.
    Values,
    // Operations that change the map (their receiver is a place):
    Insert,
    /// `Option[V]`: removes a key and returns its value.
    Remove,
    Clear,
}

impl MapOp {
    pub fn changes_map(self) -> bool {
        matches!(self, MapOp::Insert | MapOp::Remove | MapOp::Clear)
    }
}

/// What a list or map operation works on.
#[derive(Clone, Debug)]
pub enum ListReceiver {
    Value(Box<CheckedExpr>),
    /// For operations that change the list.
    Place(CheckedPlace),
}

#[derive(Clone, Debug)]
pub struct CheckedArm {
    pub pattern: CheckedPattern,
    /// `if guard`: the arm only runs when this is also true.
    pub guard: Option<CheckedExpr>,
    pub body: Vec<CheckedStmt>,
    /// In a `match` used as a value: the arm's value, after `body` (`None`
    /// when the arm returns or breaks instead).
    pub value: Option<CheckedExpr>,
}

#[derive(Clone, Debug)]
pub enum CheckedPattern {
    Wildcard,
    /// Matches anything and stores the value in a local.
    Binding(LocalId),
    /// Variant `index` of the matched enum. Each payload value, of the
    /// given type, is matched by a nested pattern.
    Variant {
        index: usize,
        fields: Vec<(CheckedPattern, Type)>,
    },
    Int(i64),
    Str(String),
    Bool(bool),
    /// Matches when any alternative matches. Every alternative binds the
    /// same locals.
    Or(Vec<CheckedPattern>),
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
    /// The bits of an `f64` literal.
    Float(u64),
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
        /// The type arguments of a generic function (empty otherwise).
        type_args: Vec<Type>,
        args: Vec<CheckedExpr>,
    },
    /// A call of a `mut self` method on a place: the place's value is passed
    /// as `self` and replaced by the changed `self` afterwards.
    CallMut {
        func: FnId,
        type_args: Vec<Type>,
        place: CheckedPlace,
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
    /// A built-in operation with a non-primitive type; string methods pass
    /// the string as the first argument.
    Intrinsic {
        op: Intrinsic,
        args: Vec<CheckedExpr>,
    },
    /// The struct type is the expression's type.
    StructLit {
        fields: Vec<(usize, CheckedExpr)>,
    },
    /// Variant `index` of the enum that is the expression's type.
    Variant {
        index: usize,
        args: Vec<CheckedExpr>,
    },
    /// `operand?` on an `Option` or `Result`: the payload of `Some`/`Ok`;
    /// `None`/`Err` is returned from the function (as the function's own
    /// `Option`/`Result` type).
    /// A struct, enum, list or map as text, the way `print` shows it.
    Show(Box<CheckedExpr>),
    /// A `fn` expression: a function value of `func` (which shares the
    /// enclosing function's type parameters), with copies of these locals.
    Closure {
        func: FnId,
        captures: Vec<LocalId>,
    },
    /// A named function used as a value.
    FnRef(FnId),
    /// `receiver.method(args)` where the receiver's type is a type
    /// parameter: the method of the concrete type, found when lowering.
    TraitCall {
        method: String,
        receiver: Box<CheckedExpr>,
        args: Vec<CheckedExpr>,
    },
    /// `if cond { ... } else { ... }` as a value. Each branch runs its
    /// statements, then gives its value (`None` when it returns or breaks
    /// instead).
    IfValue {
        cond: Box<CheckedExpr>,
        then_body: Vec<CheckedStmt>,
        then_value: Option<Box<CheckedExpr>>,
        else_body: Vec<CheckedStmt>,
        else_value: Option<Box<CheckedExpr>>,
    },
    /// `match` as a value; each arm has its `value`.
    MatchValue {
        scrutinee: Box<CheckedExpr>,
        arms: Vec<CheckedArm>,
    },
    /// A call of a function value.
    CallValue {
        callee: Box<CheckedExpr>,
        args: Vec<CheckedExpr>,
    },
    /// `to_i32(x)`, `to_u8(x)`, `to_u64(x)` or `to_int(x)` of an integer:
    /// the same number as the expression's type, which must fit.
    IntCast(Box<CheckedExpr>),
    /// `Box(value)`: the value moved to the heap.
    BoxNew(Box<CheckedExpr>),
    /// `b.value()`: the value in a box.
    BoxValue(Box<CheckedExpr>),
    Try {
        operand: Box<CheckedExpr>,
        /// When the error type differs from the function's: the function's
        /// error enum and the variant that wraps the operand's error.
        convert: Option<(Type, usize)>,
    },
    /// `[a, b, c]`; the list type is the expression's type.
    ListLit(Vec<CheckedExpr>),
    /// `base[index]` on a list, or `base[key]` on a map.
    Index {
        base: Box<CheckedExpr>,
        index: Box<CheckedExpr>,
    },
    /// `[key: value, ...]`; the map type is the expression's type.
    MapLit(Vec<(CheckedExpr, CheckedExpr)>),
    /// A built-in map method: `m.get(k)`, `m.insert(k, v)`, ...
    MapOp {
        op: MapOp,
        receiver: ListReceiver,
        args: Vec<CheckedExpr>,
    },
    /// A built-in list method: `xs.len()`, `xs.push(x)`, ...
    ListOp {
        op: ListOp,
        receiver: ListReceiver,
        args: Vec<CheckedExpr>,
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
    /// `-x` on `f64`.
    FloatNeg,
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
    /// Byte-wise string comparisons.
    StrLt,
    StrLe,
    StrGt,
    StrGe,
    FloatAdd,
    FloatSub,
    FloatMul,
    FloatDiv,
    FloatRem,
    FloatEq,
    FloatNe,
    FloatLt,
    FloatLe,
    FloatGt,
    FloatGe,
    /// `==` on structs, enums, lists and maps: equal when every value in
    /// them is equal. Maps are equal when they have the same entries, in
    /// any order.
    ValueEq,
    ValueNe,
}

/// Built-in operations that lower to more than one runtime call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intrinsic {
    /// `s.is_empty()`
    StrIsEmpty,
    /// `string_from_bytes(b)`: `Option[string]`.
    StrFromBytes,
    /// `s.find(t)`: `Option[i64]`.
    StrFind,
    /// `s.parse_int()`: `Option[i64]`.
    ParseInt,
    ParseFloat,
    /// `read_line()`: `Option[string]`.
    ReadLine,
    /// `read_file(path)`: `Result[string, string]`.
    ReadFile,
    /// `write_file(path, text)`: `Result[i64, string]`.
    WriteFile,
    /// `args()`: `List[string]`.
    Args,
    /// `env(name)`: `Option[string]`.
    Env,
}
