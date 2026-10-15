//! Lowering from a [`CheckedProgram`] to a Brim IR [`Module`].
//!
//! Lowering is total: every checked program lowers to a module that passes
//! [`brim_ir::validate`]. Any failure here is a compiler bug, not a user error.
//!
//! Translation scheme:
//! - every local variable (including parameters) gets one slot;
//! - expressions become values in the current block;
//! - `&&` / `||` short-circuit through a temporary `bool` slot and three
//!   blocks (evaluate left, maybe evaluate right, join);
//! - string `+`, `==` and `!=` become runtime calls;
//! - `if` becomes a branch to a `then` and an `else` block that both jump to
//!   a join block;
//! - `while` becomes a header block (evaluates the condition), a body block
//!   that jumps back to the header, and an exit block; `break` jumps to the
//!   exit and `continue` to the header;
//! - statements after a `return`, `break` or `continue` are unreachable and
//!   are not lowered.
//!
//! Generics are **monomorphized**: every generic function is lowered once
//! per combination of concrete type arguments it is called with, as an IR
//! function named `name[args]`. Non-generic functions are lowered in source
//! order first; instances are added as calls to them are found.
//!
//! Lists lower to IR list types. Changing part of a variable (`xs.push(x)`,
//! `p.items[i] = v`) takes the variable's value out of its slot, takes each
//! part on the path out of its parent (leaving a zero value), changes the
//! innermost part and puts everything back. Every step consumes its operand,
//! so a list or struct that is not shared is changed in place.
//!
//! Every concrete struct type becomes an IR struct. An enum becomes an IR
//! struct too: field 0 is the variant's index (the *tag*), followed by the
//! payload fields of every variant in turn. A value only uses its own
//! variant's fields; the others hold zero values. `match` loads the tag and
//! branches on it.

use std::collections::{HashMap, VecDeque};

use brim_ir::{
    Block, BlockId, Const, Field, FuncId, Function, Inst, InstKind, ListId, MapId, Module, Slot,
    SlotId, Span, StructDef, StructId, Terminator, Type as IrType, ValueId,
};
use brim_runtime::RuntimeFn;
use brim_semantic::checked::{
    BinaryOp, CheckedArm, CheckedExpr, CheckedExprKind, CheckedFn, CheckedForIter, CheckedPattern,
    CheckedPlace, CheckedProgram, CheckedStmt, Intrinsic, ListOp, ListReceiver, MapOp, PlaceStep,
    UnaryOp,
};
use brim_semantic::{Def, Type, TypeTable};

/// Lowers a checked program to IR.
pub fn lower(program: &CheckedProgram) -> Module {
    lower_tests(program).0
}

/// Lowers a checked program to IR, together with the IR functions of its
/// tests ([`CheckedProgram::tests`]). Without `main`, the module's entry is
/// an empty function.
pub fn lower_tests(program: &CheckedProgram) -> (Module, Vec<FuncId>) {
    let mut m = ModuleLowerer {
        program,
        types: program.types.clone(),
        module: Module {
            structs: Vec::new(),
            lists: Vec::new(),
            maps: Vec::new(),
            functions: Vec::new(),
            entry: FuncId(0),
        },
        ir_types: HashMap::new(),
        enum_layouts: HashMap::new(),
        instances: HashMap::new(),
        queue: VecDeque::new(),
        pairs: HashMap::new(),
    };
    // Only functions reachable from `main` and the tests are lowered: an
    // imported module's unused functions cost nothing (and need nothing,
    // such as libcurl for `std.http`'s HTTPS client).
    m.module.entry = match program.entry {
        Some(entry) => m.instance(entry.0, Vec::new()),
        None => {
            m.module.functions.push(Function {
                name: "<tests>".into(),
                consumes_self: false,
                param_count: 0,
                ret: IrType::Void,
                slots: Vec::new(),
                values: Vec::new(),
                blocks: vec![Block {
                    insts: Vec::new(),
                    term: Terminator::Return(None),
                }],
            });
            func_id(m.module.functions.len() - 1)
        }
    };
    let tests = program
        .tests
        .iter()
        .map(|t| m.instance(t.0, Vec::new()))
        .collect();
    while let Some((id, index, type_args)) = m.queue.pop_front() {
        let mut function = FnLowerer::lower(&mut m, &program.functions[index], type_args);
        function.name = std::mem::take(&mut m.module.functions[id.index()].name);
        m.module.functions[id.index()] = function;
    }
    (m.module, tests)
}

fn func_id(index: usize) -> FuncId {
    FuncId(u32::try_from(index).expect("function count fits in u32"))
}

/// A part of a place that was moved out of its parent, for putting it back.
struct Parent {
    /// The parent, with a zero value where the part was.
    value: ValueId,
    ty: IrType,
    at: At,
}

/// Where in its parent a part of a place is.
#[derive(Clone, Copy)]
enum At {
    Field(usize),
    ListElement(ValueId),
    /// The entry at this position of a map.
    MapEntry(ValueId),
}

/// Where each variant's payload lives in an enum's IR struct.
#[derive(Clone)]
struct EnumLayout {
    /// Index of the first payload field of each variant.
    offsets: Vec<usize>,
}

struct ModuleLowerer<'p> {
    program: &'p CheckedProgram,
    /// The program's types, extended with instantiations made here.
    types: TypeTable,
    module: Module,
    /// IR types of concrete named types.
    ir_types: HashMap<Type, IrType>,
    enum_layouts: HashMap<Type, EnumLayout>,
    /// `(function, type arguments)` to its IR function.
    instances: HashMap<(usize, Vec<Type>), FuncId>,
    /// Instances whose bodies still have to be lowered.
    queue: VecDeque<(FuncId, usize, Vec<Type>)>,
    /// Result-and-`self` structs of `mut self` methods.
    pairs: HashMap<(IrType, IrType), StructId>,
}

impl ModuleLowerer<'_> {
    /// The IR function for `function` with `type_args`, queued for lowering
    /// the first time it is requested.
    fn instance(&mut self, function: usize, type_args: Vec<Type>) -> FuncId {
        let key = (function, type_args);
        if let Some(id) = self.instances.get(&key) {
            return *id;
        }
        let id = func_id(self.module.functions.len());
        let f = &self.program.functions[function];
        let name = if key.1.is_empty() {
            f.name.clone()
        } else {
            let args: Vec<String> = key.1.iter().map(|t| self.type_name(*t)).collect();
            format!("{}[{}]", f.name, args.join(", "))
        };
        // A placeholder until the queue reaches it.
        self.module.functions.push(Function {
            name,
            consumes_self: false,
            param_count: 0,
            ret: IrType::Void,
            slots: Vec::new(),
            values: Vec::new(),
            blocks: Vec::new(),
        });
        self.instances.insert(key.clone(), id);
        self.queue.push_back((id, key.0, key.1));
        id
    }

    /// A concrete type as written in source, for IR names.
    fn type_name(&self, ty: Type) -> String {
        match ty {
            Type::Named(id) => {
                let named = self.types.get(id);
                let name = match named.def {
                    Def::Struct(s) => &self.program.struct_def(s).name,
                    Def::Enum(e) => &self.program.enum_def(e).name,
                    Def::List => "List",
                    Def::Map => "Map",
                };
                if named.args.is_empty() {
                    name.to_string()
                } else {
                    let args: Vec<String> = named.args.iter().map(|t| self.type_name(*t)).collect();
                    format!("{name}[{}]", args.join(", "))
                }
            }
            other => other.to_string(),
        }
    }

    /// The IR type of a concrete (parameter-free) type.
    fn ir_type(&mut self, ty: Type) -> IrType {
        match ty {
            Type::I64 => IrType::I64,
            Type::F64 => IrType::F64,
            Type::Bool => IrType::Bool,
            Type::String => IrType::Str,
            Type::Void => IrType::Void,
            Type::Named(id) => {
                if let Some(ir) = self.ir_types.get(&ty) {
                    return *ir;
                }
                let named = self.types.get(id).clone();
                // Types can contain themselves through lists and maps
                // (`struct Node { children: List[Node] }`), so each type's
                // id is reserved before its contents are lowered.
                if named.def == Def::Map {
                    let id = MapId(
                        u32::try_from(self.module.maps.len()).expect("map count fits in u32"),
                    );
                    self.module.maps.push((IrType::Void, IrType::Void));
                    let ir = IrType::Map(id);
                    self.ir_types.insert(ty, ir);
                    let key = self.ir_type(named.args[0]);
                    let value = self.ir_type(named.args[1]);
                    self.module.maps[id.index()] = (key, value);
                    return ir;
                }
                if named.def == Def::List {
                    let id = ListId(
                        u32::try_from(self.module.lists.len()).expect("list count fits in u32"),
                    );
                    self.module.lists.push(IrType::Void);
                    let ir = IrType::List(id);
                    self.ir_types.insert(ty, ir);
                    let elem = self.ir_type(named.args[0]);
                    self.module.lists[id.index()] = elem;
                    return ir;
                }
                let id = StructId(
                    u32::try_from(self.module.structs.len()).expect("struct count fits in u32"),
                );
                self.module.structs.push(StructDef {
                    name: self.type_name(ty),
                    fields: Vec::new(),
                });
                let ir = IrType::Struct(id);
                self.ir_types.insert(ty, ir);
                let mut fields = Vec::new();
                match named.def {
                    Def::Struct(s) => {
                        let def = self.program.struct_def(s);
                        for field in &def.fields {
                            let field_ty = self.types.subst(field.ty, &named.args);
                            fields.push(Field {
                                name: field.name.clone(),
                                ty: self.ir_type(field_ty),
                            });
                        }
                    }
                    Def::Enum(e) => {
                        let def = self.program.enum_def(e);
                        fields.push(Field {
                            name: "tag".into(),
                            ty: IrType::I64,
                        });
                        let mut offsets = Vec::with_capacity(def.variants.len());
                        for variant in &def.variants {
                            offsets.push(fields.len());
                            for (j, field) in variant.fields.iter().enumerate() {
                                let field_ty = self.types.subst(*field, &named.args);
                                fields.push(Field {
                                    name: format!("{}.{j}", variant.name),
                                    ty: self.ir_type(field_ty),
                                });
                            }
                        }
                        self.enum_layouts.insert(ty, EnumLayout { offsets });
                    }
                    Def::List | Def::Map => unreachable!("handled above"),
                }
                self.module.structs[id.index()].fields = fields;
                ir
            }
            Type::Param(_) | Type::Error => {
                unreachable!("lowering only sees concrete, error-free types")
            }
        }
    }

    fn enum_layout(&mut self, ty: Type) -> EnumLayout {
        self.ir_type(ty);
        self.enum_layouts
            .get(&ty)
            .expect("an enum type has a layout")
            .clone()
    }
}

/// Whether lowering `expr` creates new blocks (it contains `&&` or `||`).
fn branches(expr: &CheckedExpr) -> bool {
    match &expr.kind {
        CheckedExprKind::And(..) | CheckedExprKind::Or(..) | CheckedExprKind::Try { .. } => true,
        CheckedExprKind::Int(_)
        | CheckedExprKind::Float(_)
        | CheckedExprKind::Bool(_)
        | CheckedExprKind::Str(_)
        | CheckedExprKind::Local(_) => false,
        CheckedExprKind::Unary { operand, .. } => branches(operand),
        CheckedExprKind::Binary { lhs, rhs, .. } => branches(lhs) || branches(rhs),
        CheckedExprKind::CallMut { .. } => true,
        // Most intrinsics build an `Option` or `Result` with a branch.
        CheckedExprKind::Intrinsic { .. } => true,
        CheckedExprKind::Call { args, .. }
        | CheckedExprKind::Builtin { args, .. }
        | CheckedExprKind::Variant { args, .. } => args.iter().any(branches),
        CheckedExprKind::StructLit { fields } => fields.iter().any(|(_, v)| branches(v)),
        CheckedExprKind::ListLit(items) => items.iter().any(branches),
        CheckedExprKind::Index { base, index } => branches(base) || branches(index),
        // Some list and map operations branch themselves (`get`, `pop`, ...).
        CheckedExprKind::ListOp { .. } | CheckedExprKind::MapOp { .. } => true,
        CheckedExprKind::MapLit(entries) => entries.iter().any(|(k, v)| branches(k) || branches(v)),
        CheckedExprKind::Field { base, .. } => branches(base),
    }
}

struct FnLowerer<'m, 'p> {
    m: &'m mut ModuleLowerer<'p>,
    /// The type arguments of this instance.
    type_args: Vec<Type>,
    /// The function's concrete return type.
    ret: Type,
    /// For `mut self` methods: how `self` goes back to the caller (alone,
    /// or in a pair struct with the result).
    self_return: Option<Option<StructId>>,
    func: Function,
    current: BlockId,
    /// Whether the current block already has its terminator.
    terminated: bool,
    /// Innermost last: `(continue target, break target)` of enclosing loops.
    loops: Vec<(BlockId, BlockId)>,
}

impl<'m, 'p> FnLowerer<'m, 'p> {
    fn lower(m: &'m mut ModuleLowerer<'p>, f: &CheckedFn, type_args: Vec<Type>) -> Function {
        let ret = m.types.subst(f.ret, &type_args);
        let mut lowerer = FnLowerer {
            m,
            type_args,
            ret,
            self_return: None,
            func: Function {
                consumes_self: false,
                name: String::new(),
                param_count: f.param_count,
                ret: IrType::Void,
                slots: Vec::new(),
                values: Vec::new(),
                blocks: Vec::new(),
            },
            current: BlockId(0),
            terminated: false,
            loops: Vec::new(),
        };
        lowerer.func.ret = lowerer.ir_type(f.ret);
        for local in &f.locals {
            let ty = lowerer.ir_type(local.ty);
            lowerer.func.slots.push(Slot {
                name: local.name.clone(),
                ty,
            });
        }
        if f.mut_self {
            // `self` goes back to the caller, with the result if there is one.
            let self_ty = lowerer.func.slots[0].ty;
            lowerer.func.consumes_self = true;
            lowerer.self_return = Some(match lowerer.func.ret {
                IrType::Void => {
                    lowerer.func.ret = self_ty;
                    None
                }
                ret => {
                    let pair = lowerer.m.pair_type(ret, self_ty);
                    lowerer.func.ret = IrType::Struct(pair);
                    Some(pair)
                }
            });
        }
        lowerer.current = lowerer.new_block();
        lowerer.stmts(&f.body);
        if !lowerer.terminated {
            // Semantic analysis guarantees non-void functions always return.
            if f.ret == Type::Void {
                lowerer.return_value(None, f.span);
            } else {
                lowerer.terminate(Terminator::Unreachable);
            }
        }
        lowerer.func
    }

    /// Returns `value`; a `mut self` method also returns `self`.
    fn return_value(&mut self, value: Option<ValueId>, span: Span) {
        let value = match self.self_return {
            None => value,
            Some(pair) => {
                let self_ty = self.func.slots[0].ty;
                let current = self.emit_value(InstKind::Load(SlotId(0)), self_ty, span);
                Some(match (pair, value) {
                    (Some(pair), Some(v)) => self.emit_value(
                        InstKind::MakeStruct(pair, vec![v, current]),
                        IrType::Struct(pair),
                        span,
                    ),
                    _ => current,
                })
            }
        };
        self.terminate(Terminator::Return(value));
    }

    // ---- types ----------------------------------------------------------

    /// `ty` with this instance's type arguments filled in.
    fn concrete(&mut self, ty: Type) -> Type {
        if self.type_args.is_empty() {
            ty
        } else {
            self.m.types.subst(ty, &self.type_args)
        }
    }

    fn ir_type(&mut self, ty: Type) -> IrType {
        let ty = self.concrete(ty);
        self.m.ir_type(ty)
    }

    // ---- building blocks ----------------------------------------------

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.func.blocks.len() as u32);
        self.func.blocks.push(Block {
            insts: Vec::new(),
            term: Terminator::Unreachable,
        });
        id
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current = block;
        self.terminated = false;
    }

    fn terminate(&mut self, term: Terminator) {
        debug_assert!(!self.terminated);
        self.func.blocks[self.current.index()].term = term;
        self.terminated = true;
    }

    fn new_slot(&mut self, name: &str, ty: IrType) -> SlotId {
        let id = SlotId(self.func.slots.len() as u32);
        self.func.slots.push(Slot {
            name: name.to_string(),
            ty,
        });
        id
    }

    /// Appends an instruction. `ty` is the type of its result, if any.
    fn emit(&mut self, kind: InstKind, ty: Option<IrType>, span: Span) -> Option<ValueId> {
        let result = ty.filter(|t| *t != IrType::Void).map(|t| {
            let id = ValueId(self.func.values.len() as u32);
            self.func.values.push(t);
            id
        });
        self.func.blocks[self.current.index()].insts.push(Inst {
            result,
            kind,
            span: Some(span),
        });
        result
    }

    fn emit_value(&mut self, kind: InstKind, ty: IrType, span: Span) -> ValueId {
        self.emit(kind, Some(ty), span)
            .expect("non-void instructions produce a value")
    }

    /// A zero value of `ty`, for the unused payload fields of an enum.
    fn zero(&mut self, ty: IrType, span: Span) -> ValueId {
        let kind = match ty {
            IrType::I64 => InstKind::Const(Const::I64(0)),
            IrType::F64 => InstKind::Const(Const::F64(0)),
            IrType::Bool => InstKind::Const(Const::Bool(false)),
            IrType::Str => InstKind::Const(Const::Str(String::new())),
            IrType::Struct(id) => {
                let fields: Vec<IrType> = self.m.module.structs[id.index()]
                    .fields
                    .iter()
                    .map(|f| f.ty)
                    .collect();
                let values = fields.into_iter().map(|f| self.zero(f, span)).collect();
                InstKind::MakeStruct(id, values)
            }
            IrType::List(id) => InstKind::ListNew(id, Vec::new()),
            IrType::Map(id) => InstKind::MapNew(id),
            IrType::Void => unreachable!("fields are never void"),
        };
        self.emit_value(kind, ty, span)
    }

    // ---- statements ---------------------------------------------------

    fn stmts(&mut self, stmts: &[CheckedStmt]) {
        for stmt in stmts {
            if self.terminated {
                // Unreachable code (already warned about) is not lowered.
                return;
            }
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &CheckedStmt) {
        match stmt {
            CheckedStmt::Let { local, init } => {
                let value = self.value(init);
                let slot = SlotId(local.0 as u32);
                self.emit(InstKind::Store(slot, value), None, init.span);
            }
            CheckedStmt::Expr(expr) => {
                self.expr(expr);
                // `exit(...)` does not return.
                if let CheckedExprKind::Builtin { runtime, .. } = &expr.kind
                    && runtime.diverges()
                {
                    self.terminate(Terminator::Unreachable);
                }
            }
            CheckedStmt::Return { value, span } => {
                let value = value.as_ref().map(|v| self.value(v));
                self.return_value(value, *span);
            }
            CheckedStmt::Block(stmts) => self.stmts(stmts),
            CheckedStmt::Assign { place, op, value } => self.assign(place, *op, value),
            CheckedStmt::For { var, iter, body } => self.for_stmt(*var, iter, body),
            CheckedStmt::If {
                cond,
                then_body,
                else_body,
            } => self.if_stmt(cond, then_body, else_body),
            CheckedStmt::While { cond, body } => self.while_stmt(cond, body),
            CheckedStmt::Match { scrutinee, arms } => self.match_stmt(scrutinee, arms),
            CheckedStmt::Break => {
                let (_, exit) = *self.loops.last().expect("checked: break is inside a loop");
                self.terminate(Terminator::Jump(exit));
            }
            CheckedStmt::Continue => {
                let (header, _) = *self
                    .loops
                    .last()
                    .expect("checked: continue is inside a loop");
                self.terminate(Terminator::Jump(header));
            }
        }
    }

    // ---- places ----------------------------------------------------------

    /// Evaluates the index expressions of a place, left to right; `None` for
    /// field steps.
    fn place_indices(&mut self, place: &CheckedPlace) -> Vec<Option<ValueId>> {
        let exprs: Vec<&CheckedExpr> = place
            .steps
            .iter()
            .filter_map(|s| match s {
                PlaceStep::Index(e) => Some(e),
                PlaceStep::Field(_) => None,
            })
            .collect();
        let mut values = self.values(&exprs).into_iter();
        place
            .steps
            .iter()
            .map(|s| match s {
                PlaceStep::Index(_) => values.next(),
                PlaceStep::Field(_) => None,
            })
            .collect()
    }

    /// The current value of a place, given its evaluated indices (list
    /// indices or map keys).
    fn read_place(
        &mut self,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        span: Span,
    ) -> ValueId {
        let slot = SlotId(place.local.0 as u32);
        let mut ty = self.func.slots[slot.index()].ty;
        let mut value = self.emit_value(InstKind::Load(slot), ty, span);
        for (step, index) in place.steps.iter().zip(indices) {
            let (child, child_ty) = match (step, ty) {
                (PlaceStep::Field(i), _) => {
                    let child_ty = self.field_type(ty, *i);
                    let child = self.emit_value(InstKind::Extract(value, *i), child_ty, span);
                    (child, child_ty)
                }
                (PlaceStep::Index(_), IrType::Map(id)) => {
                    let key = index.expect("index steps have an index");
                    let (_, value_ty) = self.m.module.maps[id.index()];
                    let at = self.emit_value(InstKind::MapIndex(value, key), IrType::I64, span);
                    let child = self.emit_value(InstKind::MapValueAt(value, at), value_ty, span);
                    (child, value_ty)
                }
                (PlaceStep::Index(_), _) => {
                    let index = index.expect("index steps have an index");
                    let child_ty = self.list_elem(ty);
                    let child = self.emit_value(InstKind::ListGet(value, index), child_ty, span);
                    (child, child_ty)
                }
            };
            value = child;
            ty = child_ty;
        }
        value
    }

    /// Moves the parts of a place out of their parents, down to the part
    /// before step `upto`. Returns that part, its type, and the parents
    /// (with holes) to put back afterwards.
    fn descend(
        &mut self,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        upto: usize,
        span: Span,
    ) -> (ValueId, IrType, Vec<Parent>) {
        let slot = SlotId(place.local.0 as u32);
        let mut ty = self.func.slots[slot.index()].ty;
        let mut current = self.emit_value(InstKind::Take(slot), ty, span);
        let mut parents = Vec::new();
        for (step, index) in place.steps[..upto].iter().zip(indices) {
            let (child, child_ty, parent, at) = match (step, ty) {
                (PlaceStep::Field(i), _) => {
                    let child_ty = self.field_type(ty, *i);
                    let child = self.emit_value(InstKind::Extract(current, *i), child_ty, span);
                    let hole = self.zero(child_ty, span);
                    let parent = self.emit_value(InstKind::Insert(current, *i, hole), ty, span);
                    (child, child_ty, parent, At::Field(*i))
                }
                (PlaceStep::Index(_), IrType::Map(id)) => {
                    let key = index.expect("index steps have an index");
                    let (_, value_ty) = self.m.module.maps[id.index()];
                    let pos = self.emit_value(InstKind::MapIndex(current, key), IrType::I64, span);
                    let child = self.emit_value(InstKind::MapValueAt(current, pos), value_ty, span);
                    let hole = self.zero(value_ty, span);
                    let parent =
                        self.emit_value(InstKind::MapSetValueAt(current, pos, hole), ty, span);
                    (child, value_ty, parent, At::MapEntry(pos))
                }
                (PlaceStep::Index(_), _) => {
                    let index = index.expect("index steps have an index");
                    let child_ty = self.list_elem(ty);
                    let child = self.emit_value(InstKind::ListGet(current, index), child_ty, span);
                    let hole = self.zero(child_ty, span);
                    let parent = self.emit_value(InstKind::ListSet(current, index, hole), ty, span);
                    (child, child_ty, parent, At::ListElement(index))
                }
            };
            parents.push(Parent {
                value: parent,
                ty,
                at,
            });
            current = child;
            ty = child_ty;
        }
        (current, ty, parents)
    }

    /// Puts `new` back into the parents left by [`descend`](Self::descend)
    /// and stores the result in the place's variable.
    fn ascend(&mut self, place: &CheckedPlace, parents: Vec<Parent>, new: ValueId, span: Span) {
        let mut new = new;
        for parent in parents.into_iter().rev() {
            let kind = match parent.at {
                At::Field(i) => InstKind::Insert(parent.value, i, new),
                At::ListElement(index) => InstKind::ListSet(parent.value, index, new),
                At::MapEntry(pos) => InstKind::MapSetValueAt(parent.value, pos, new),
            };
            new = self.emit_value(kind, parent.ty, span);
        }
        let slot = SlotId(place.local.0 as u32);
        self.emit(InstKind::Store(slot, new), None, span);
    }

    /// Replaces the value of a place with `change(current value)`. The
    /// current value and the parts around it are moved out first, so the
    /// change can happen in place. `change` must not branch. A map entry
    /// on the path must exist.
    fn write_place(
        &mut self,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        span: Span,
        change: impl FnOnce(&mut Self, ValueId) -> ValueId,
    ) {
        let (current, _, parents) = self.descend(place, indices, place.steps.len(), span);
        let new = change(self, current);
        self.ascend(place, parents, new, span);
    }

    /// Sets a place to `value` without reading it first, so `map[key] = v`
    /// can add a new key.
    fn replace_place(
        &mut self,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        value: ValueId,
        span: Span,
    ) {
        let last = place.steps.len() - 1;
        let (parent, parent_ty, parents) = self.descend(place, indices, last, span);
        let kind = match (&place.steps[last], parent_ty) {
            (PlaceStep::Field(i), _) => InstKind::Insert(parent, *i, value),
            (PlaceStep::Index(_), IrType::Map(_)) => {
                InstKind::MapInsert(parent, indices[last].expect("an index"), value)
            }
            (PlaceStep::Index(_), _) => {
                InstKind::ListSet(parent, indices[last].expect("an index"), value)
            }
        };
        let new = self.emit_value(kind, parent_ty, span);
        self.ascend(place, parents, new, span);
    }

    fn assign(&mut self, place: &CheckedPlace, op: Option<BinaryOp>, value: &CheckedExpr) {
        let span = value.span;
        let slot = SlotId(place.local.0 as u32);
        if place.steps.is_empty() {
            let v = match op {
                None => self.value(value),
                Some(op) => {
                    // `x op= value` on a plain variable: read, combine, store.
                    let current = CheckedExpr {
                        kind: CheckedExprKind::Local(place.local),
                        ty: place.ty,
                        span,
                    };
                    let [a, b] = self
                        .values(&[&current, value])
                        .try_into()
                        .expect("two operands");
                    self.binary(op, a, b, span)
                }
            };
            self.emit(InstKind::Store(slot, v), None, span);
            return;
        }
        let indices = self.place_indices(place);
        let v = if indices.iter().any(Option::is_some) && branches(value) {
            // Keep the indices usable after `value` ends the block.
            let spilled = self.spill(&indices, span);
            let v = self.value(value);
            let indices = self.reload(&spilled, span);
            return self.finish_assign(place, op, &indices, v, span);
        } else {
            self.value(value)
        };
        self.finish_assign(place, op, &indices, v, span);
    }

    fn finish_assign(
        &mut self,
        place: &CheckedPlace,
        op: Option<BinaryOp>,
        indices: &[Option<ValueId>],
        v: ValueId,
        span: Span,
    ) {
        match op {
            None => self.replace_place(place, indices, v, span),
            Some(op) => self.write_place(place, indices, span, |s, current| {
                s.binary(op, current, v, span)
            }),
        }
    }

    /// Stores index values in slots, so they survive a block change.
    fn spill(&mut self, indices: &[Option<ValueId>], span: Span) -> Vec<Option<SlotId>> {
        indices
            .iter()
            .map(|v| {
                v.map(|v| {
                    // List indices are `i64`; map keys can be any key type.
                    let ty = self.func.values[v.index()];
                    let slot = self.new_slot("index", ty);
                    self.emit(InstKind::Store(slot, v), None, span);
                    slot
                })
            })
            .collect()
    }

    fn reload(&mut self, slots: &[Option<SlotId>], span: Span) -> Vec<Option<ValueId>> {
        slots
            .iter()
            .map(|s| {
                s.map(|s| {
                    let ty = self.func.slots[s.index()].ty;
                    self.emit_value(InstKind::Load(s), ty, span)
                })
            })
            .collect()
    }

    fn list_elem(&self, ty: IrType) -> IrType {
        let IrType::List(id) = ty else {
            unreachable!("checked: only lists are indexed");
        };
        self.m.module.lists[id.index()]
    }

    // ---- loops -----------------------------------------------------------

    fn for_stmt(
        &mut self,
        var: brim_semantic::checked::LocalId,
        iter: &CheckedForIter,
        body: &[CheckedStmt],
    ) {
        let var = SlotId(var.0 as u32);
        let counter = self.new_slot("i", IrType::I64);
        let end = self.new_slot("end", IrType::I64);
        let span;
        let list = match iter {
            CheckedForIter::Range(start, stop) => {
                span = start.span;
                let [a, b] = self.values(&[start, stop]).try_into().expect("two bounds");
                self.emit(InstKind::Store(counter, a), None, span);
                self.emit(InstKind::Store(end, b), None, span);
                None
            }
            CheckedForIter::Each(list) => {
                span = list.span;
                let ty = self.ir_type(list.ty);
                let value = self.value(list);
                let slot = self.new_slot("list", ty);
                self.emit(InstKind::Store(slot, value), None, span);
                let len = self.emit_value(InstKind::ListLen(value), IrType::I64, span);
                let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
                self.emit(InstKind::Store(counter, zero), None, span);
                self.emit(InstKind::Store(end, len), None, span);
                Some((slot, ty))
            }
        };
        let header = self.new_block();
        let body_block = self.new_block();
        let next = self.new_block();
        let exit = self.new_block();
        self.terminate(Terminator::Jump(header));

        self.switch_to(header);
        let i = self.emit_value(InstKind::Load(counter), IrType::I64, span);
        let n = self.emit_value(InstKind::Load(end), IrType::I64, span);
        let more = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::ILt, i, n),
            IrType::Bool,
            span,
        );
        self.terminate(Terminator::Branch {
            cond: more,
            then_block: body_block,
            else_block: exit,
        });

        self.switch_to(body_block);
        let i = self.emit_value(InstKind::Load(counter), IrType::I64, span);
        let item = match list {
            None => i,
            Some((slot, ty)) => {
                let l = self.emit_value(InstKind::Load(slot), ty, span);
                let elem = self.list_elem(ty);
                self.emit_value(InstKind::ListGet(l, i), elem, span)
            }
        };
        self.emit(InstKind::Store(var, item), None, span);
        self.loops.push((next, exit));
        self.stmts_then_jump(body, next);
        self.loops.pop();

        // `i + 1` cannot overflow: `i < end <= i64::MAX`.
        self.switch_to(next);
        let i = self.emit_value(InstKind::Load(counter), IrType::I64, span);
        let one = self.emit_value(InstKind::Const(Const::I64(1)), IrType::I64, span);
        let i = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IAdd, i, one),
            IrType::I64,
            span,
        );
        self.emit(InstKind::Store(counter, i), None, span);
        self.terminate(Terminator::Jump(header));

        self.switch_to(exit);
    }

    // ---- list operations ---------------------------------------------------

    fn list_op(
        &mut self,
        op: ListOp,
        receiver: &ListReceiver,
        args: &[CheckedExpr],
        ty: IrType,
        result_ty: brim_semantic::Type,
        span: Span,
    ) -> Option<ValueId> {
        match receiver {
            ListReceiver::Value(list) => {
                let mut operands: Vec<&CheckedExpr> = vec![list];
                operands.extend(args);
                let values = self.values(&operands);
                let l = values[0];
                let i64_ty = IrType::I64;
                Some(match op {
                    ListOp::Len => self.emit_value(InstKind::ListLen(l), i64_ty, span),
                    ListOp::IsEmpty => {
                        let len = self.emit_value(InstKind::ListLen(l), i64_ty, span);
                        let zero = self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span);
                        self.emit_value(
                            InstKind::Binary(brim_ir::BinaryOp::IEq, len, zero),
                            IrType::Bool,
                            span,
                        )
                    }
                    ListOp::Contains => {
                        self.emit_value(InstKind::ListContains(l, values[1]), ty, span)
                    }
                    ListOp::Join => self.emit_value(InstKind::ListJoin(l, values[1]), ty, span),
                    ListOp::Get | ListOp::First | ListOp::Last => {
                        let index = match op {
                            ListOp::Get => values[1],
                            ListOp::First => {
                                self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span)
                            }
                            _ => {
                                let len = self.emit_value(InstKind::ListLen(l), i64_ty, span);
                                let one =
                                    self.emit_value(InstKind::Const(Const::I64(1)), i64_ty, span);
                                self.emit_value(
                                    InstKind::Binary(brim_ir::BinaryOp::ISub, len, one),
                                    i64_ty,
                                    span,
                                )
                            }
                        };
                        let option_ty = self.concrete(result_ty);
                        self.optional_element(l, index, option_ty, span)
                    }
                    _ => unreachable!("checked: changing operations have a place receiver"),
                })
            }
            ListReceiver::Place(place) => {
                let indices = self.place_indices(place);
                let args: Vec<ValueId> = if args.iter().any(branches) {
                    let spilled = self.spill(&indices, span);
                    let values = self.values(&args.iter().collect::<Vec<_>>());
                    let indices = self.reload(&spilled, span);
                    return self.change_list(op, place, &indices, &values, ty, result_ty, span);
                } else {
                    self.values(&args.iter().collect::<Vec<_>>())
                };
                self.change_list(op, place, &indices, &args, ty, result_ty, span)
            }
        }
    }

    // ---- map operations ----------------------------------------------------

    fn map_op(
        &mut self,
        op: MapOp,
        receiver: &ListReceiver,
        args: &[CheckedExpr],
        ty: IrType,
        result_ty: brim_semantic::Type,
        span: Span,
    ) -> Option<ValueId> {
        let i64_ty = IrType::I64;
        match receiver {
            ListReceiver::Value(map) => {
                let mut operands: Vec<&CheckedExpr> = vec![map];
                operands.extend(args);
                let values = self.values(&operands);
                let m = values[0];
                Some(match op {
                    MapOp::Len => self.emit_value(InstKind::MapLen(m), i64_ty, span),
                    MapOp::IsEmpty => {
                        let len = self.emit_value(InstKind::MapLen(m), i64_ty, span);
                        let zero = self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span);
                        self.emit_value(
                            InstKind::Binary(brim_ir::BinaryOp::IEq, len, zero),
                            IrType::Bool,
                            span,
                        )
                    }
                    MapOp::ContainsKey => {
                        let at = self.emit_value(InstKind::MapFind(m, values[1]), i64_ty, span);
                        let zero = self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span);
                        self.emit_value(
                            InstKind::Binary(brim_ir::BinaryOp::IGe, at, zero),
                            IrType::Bool,
                            span,
                        )
                    }
                    MapOp::Get => {
                        let at = self.emit_value(InstKind::MapFind(m, values[1]), i64_ty, span);
                        let option_ty = self.concrete(result_ty);
                        self.optional_value(m, at, option_ty, span)
                    }
                    MapOp::Keys | MapOp::Values => self.map_entries(m, op == MapOp::Keys, ty, span),
                    _ => unreachable!("checked: changing operations have a place receiver"),
                })
            }
            ListReceiver::Place(place) => {
                let indices = self.place_indices(place);
                let (indices, args) = if args.iter().any(branches) {
                    let spilled = self.spill(&indices, span);
                    let values = self.values(&args.iter().collect::<Vec<_>>());
                    (self.reload(&spilled, span), values)
                } else {
                    let values = self.values(&args.iter().collect::<Vec<_>>());
                    (indices, values)
                };
                let map_ty = self.ir_type(place.ty);
                match op {
                    MapOp::Insert => {
                        let (k, v) = (args[0], args[1]);
                        self.write_place(place, &indices, span, |s, m| {
                            s.emit_value(InstKind::MapInsert(m, k, v), map_ty, span)
                        });
                        None
                    }
                    MapOp::Clear => {
                        let IrType::Map(id) = map_ty else {
                            unreachable!("a map place");
                        };
                        self.write_place(place, &indices, span, |s, _| {
                            s.emit_value(InstKind::MapNew(id), map_ty, span)
                        });
                        None
                    }
                    MapOp::Remove => {
                        Some(self.map_remove(place, &indices, args[0], result_ty, span))
                    }
                    _ => unreachable!("only changing operations have a place receiver"),
                }
            }
        }
    }

    /// `Some(map value at position at)` if `at >= 0`, else `None`.
    fn optional_value(
        &mut self,
        map: ValueId,
        at: ValueId,
        option_ty: brim_semantic::Type,
        span: Span,
    ) -> ValueId {
        let map_ty = self.func.values[map.index()];
        let IrType::Map(id) = map_ty else {
            unreachable!("a map operand");
        };
        let (_, value_ty) = self.m.module.maps[id.index()];
        let map_slot = self.new_slot("map", map_ty);
        self.emit(InstKind::Store(map_slot, map), None, span);
        let at_slot = self.new_slot("at", IrType::I64);
        self.emit(InstKind::Store(at_slot, at), None, span);
        let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
        let found = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IGe, at, zero),
            IrType::Bool,
            span,
        );
        let result_ir = self.m.ir_type(option_ty);
        let result = self.new_slot("value", result_ir);
        let yes = self.new_block();
        let no = self.new_block();
        let join = self.new_block();
        self.terminate(Terminator::Branch {
            cond: found,
            then_block: yes,
            else_block: no,
        });
        self.switch_to(yes);
        let map = self.emit_value(InstKind::Load(map_slot), map_ty, span);
        let at = self.emit_value(InstKind::Load(at_slot), IrType::I64, span);
        let value = self.emit_value(InstKind::MapValueAt(map, at), value_ty, span);
        let some = self.make_variant(option_ty, 0, &[value], span);
        self.emit(InstKind::Store(result, some), None, span);
        self.terminate(Terminator::Jump(join));
        self.switch_to(no);
        let none = self.make_variant(option_ty, 1, &[], span);
        self.emit(InstKind::Store(result, none), None, span);
        self.terminate(Terminator::Jump(join));
        self.switch_to(join);
        self.emit_value(InstKind::Load(result), result_ir, span)
    }

    /// The keys or values of a map, as a list in insertion order.
    fn map_entries(&mut self, map: ValueId, keys: bool, list_ty: IrType, span: Span) -> ValueId {
        let map_ty = self.func.values[map.index()];
        let IrType::Map(map_id) = map_ty else {
            unreachable!("a map operand");
        };
        let IrType::List(list_id) = list_ty else {
            unreachable!("keys and values are lists");
        };
        let (key_ty, value_ty) = self.m.module.maps[map_id.index()];
        let elem_ty = if keys { key_ty } else { value_ty };
        let map_slot = self.new_slot("map", map_ty);
        self.emit(InstKind::Store(map_slot, map), None, span);
        let out = self.new_slot("entries", list_ty);
        let empty = self.emit_value(InstKind::ListNew(list_id, Vec::new()), list_ty, span);
        self.emit(InstKind::Store(out, empty), None, span);
        let counter = self.new_slot("i", IrType::I64);
        let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
        self.emit(InstKind::Store(counter, zero), None, span);
        let header = self.new_block();
        let body = self.new_block();
        let exit = self.new_block();
        self.terminate(Terminator::Jump(header));

        self.switch_to(header);
        let i = self.emit_value(InstKind::Load(counter), IrType::I64, span);
        let m = self.emit_value(InstKind::Load(map_slot), map_ty, span);
        let len = self.emit_value(InstKind::MapLen(m), IrType::I64, span);
        let more = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::ILt, i, len),
            IrType::Bool,
            span,
        );
        self.terminate(Terminator::Branch {
            cond: more,
            then_block: body,
            else_block: exit,
        });

        self.switch_to(body);
        let i = self.emit_value(InstKind::Load(counter), IrType::I64, span);
        let m = self.emit_value(InstKind::Load(map_slot), map_ty, span);
        let kind = if keys {
            InstKind::MapKeyAt(m, i)
        } else {
            InstKind::MapValueAt(m, i)
        };
        let item = self.emit_value(kind, elem_ty, span);
        let list = self.emit_value(InstKind::Take(out), list_ty, span);
        let list = self.emit_value(InstKind::ListPush(list, item), list_ty, span);
        self.emit(InstKind::Store(out, list), None, span);
        let one = self.emit_value(InstKind::Const(Const::I64(1)), IrType::I64, span);
        let next = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IAdd, i, one),
            IrType::I64,
            span,
        );
        self.emit(InstKind::Store(counter, next), None, span);
        self.terminate(Terminator::Jump(header));

        self.switch_to(exit);
        self.emit_value(InstKind::Load(out), list_ty, span)
    }

    /// `map.remove(key)`: the removed value, if the key was there.
    fn map_remove(
        &mut self,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        key: ValueId,
        result_ty: brim_semantic::Type,
        span: Span,
    ) -> ValueId {
        let option_ty = self.concrete(result_ty);
        let result_ir = self.m.ir_type(option_ty);
        let result = self.new_slot("removed", result_ir);
        let map_ty = self.ir_type(place.ty);
        let IrType::Map(id) = map_ty else {
            unreachable!("a map place");
        };
        let (key_ty, value_ty) = self.m.module.maps[id.index()];
        let spilled = self.spill(indices, span);
        let key_slot = self.new_slot("key", key_ty);
        self.emit(InstKind::Store(key_slot, key), None, span);
        let current = self.read_place(place, indices, span);
        let at = self.emit_value(InstKind::MapFind(current, key), IrType::I64, span);
        let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
        let found = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IGe, at, zero),
            IrType::Bool,
            span,
        );
        let yes = self.new_block();
        let no = self.new_block();
        let join = self.new_block();
        self.terminate(Terminator::Branch {
            cond: found,
            then_block: yes,
            else_block: no,
        });

        self.switch_to(yes);
        let indices = self.reload(&spilled, span);
        let key = self.emit_value(InstKind::Load(key_slot), key_ty, span);
        let mut removed = None;
        self.write_place(place, &indices, span, |s, m| {
            let at = s.emit_value(InstKind::MapIndex(m, key), IrType::I64, span);
            removed = Some(s.emit_value(InstKind::MapValueAt(m, at), value_ty, span));
            s.emit_value(InstKind::MapRemoveAt(m, at), map_ty, span)
        });
        let some = self.make_variant(option_ty, 0, &[removed.expect("set")], span);
        self.emit(InstKind::Store(result, some), None, span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(no);
        let none = self.make_variant(option_ty, 1, &[], span);
        self.emit(InstKind::Store(result, none), None, span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(InstKind::Load(result), result_ir, span)
    }

    /// `Some(list[index])` if `index` is inside the list, else `None`.
    fn optional_element(
        &mut self,
        list: ValueId,
        index: ValueId,
        option_ty: brim_semantic::Type,
        span: Span,
    ) -> ValueId {
        let list_ty = self.func.values[list.index()];
        let list_slot = self.new_slot("list", list_ty);
        self.emit(InstKind::Store(list_slot, list), None, span);
        let index_slot = self.new_slot("index", IrType::I64);
        self.emit(InstKind::Store(index_slot, index), None, span);
        let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
        let not_negative = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IGe, index, zero),
            IrType::Bool,
            span,
        );
        let result_ir = self.m.ir_type(option_ty);
        let result = self.new_slot("element", result_ir);
        let check_upper = self.new_block();
        let found = self.new_block();
        let missing = self.new_block();
        let join = self.new_block();
        self.terminate(Terminator::Branch {
            cond: not_negative,
            then_block: check_upper,
            else_block: missing,
        });
        self.switch_to(check_upper);
        let index = self.emit_value(InstKind::Load(index_slot), IrType::I64, span);
        let list = self.emit_value(InstKind::Load(list_slot), list_ty, span);
        let len = self.emit_value(InstKind::ListLen(list), IrType::I64, span);
        let below_len = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::ILt, index, len),
            IrType::Bool,
            span,
        );
        self.terminate(Terminator::Branch {
            cond: below_len,
            then_block: found,
            else_block: missing,
        });

        self.switch_to(found);
        let index = self.emit_value(InstKind::Load(index_slot), IrType::I64, span);
        let list = self.emit_value(InstKind::Load(list_slot), list_ty, span);
        let elem_ty = self.list_elem(list_ty);
        let elem = self.emit_value(InstKind::ListGet(list, index), elem_ty, span);
        let some = self.make_variant(option_ty, 0, &[elem], span);
        self.emit(InstKind::Store(result, some), None, span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(missing);
        let none = self.make_variant(option_ty, 1, &[], span);
        self.emit(InstKind::Store(result, none), None, span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(InstKind::Load(result), result_ir, span)
    }

    /// An operation that changes the list at `place`.
    #[allow(clippy::too_many_arguments)]
    fn change_list(
        &mut self,
        op: ListOp,
        place: &CheckedPlace,
        indices: &[Option<ValueId>],
        args: &[ValueId],
        ty: IrType,
        result_ty: brim_semantic::Type,
        span: Span,
    ) -> Option<ValueId> {
        let list_ty = self.ir_type(place.ty);
        let elem_ty = self.list_elem(list_ty);
        match op {
            ListOp::Push => {
                let x = args[0];
                self.write_place(place, indices, span, |s, l| {
                    s.emit_value(InstKind::ListPush(l, x), list_ty, span)
                });
                None
            }
            ListOp::Insert => {
                let (i, x) = (args[0], args[1]);
                self.write_place(place, indices, span, |s, l| {
                    s.emit_value(InstKind::ListInsert(l, i, x), list_ty, span)
                });
                None
            }
            ListOp::Clear => {
                self.write_place(place, indices, span, |s, l| {
                    let zero = s.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
                    s.emit_value(InstKind::ListTruncate(l, zero), list_ty, span)
                });
                None
            }
            ListOp::Remove => {
                let i = args[0];
                let mut removed = None;
                self.write_place(place, indices, span, |s, l| {
                    removed = Some(s.emit_value(InstKind::ListGet(l, i), elem_ty, span));
                    s.emit_value(InstKind::ListRemove(l, i), list_ty, span)
                });
                debug_assert_eq!(ty, elem_ty);
                removed
            }
            ListOp::Pop => {
                let option_ty = self.concrete(result_ty);
                let result_ir = self.m.ir_type(option_ty);
                let result = self.new_slot("popped", result_ir);
                let spilled = self.spill(indices, span);
                let current = self.read_place(place, indices, span);
                let len = self.emit_value(InstKind::ListLen(current), IrType::I64, span);
                let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
                let empty = self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::IEq, len, zero),
                    IrType::Bool,
                    span,
                );
                let missing = self.new_block();
                let found = self.new_block();
                let join = self.new_block();
                self.terminate(Terminator::Branch {
                    cond: empty,
                    then_block: missing,
                    else_block: found,
                });

                self.switch_to(found);
                let indices = self.reload(&spilled, span);
                let mut popped = None;
                self.write_place(place, &indices, span, |s, l| {
                    let len = s.emit_value(InstKind::ListLen(l), IrType::I64, span);
                    let one = s.emit_value(InstKind::Const(Const::I64(1)), IrType::I64, span);
                    let last = s.emit_value(
                        InstKind::Binary(brim_ir::BinaryOp::ISub, len, one),
                        IrType::I64,
                        span,
                    );
                    popped = Some(s.emit_value(InstKind::ListGet(l, last), elem_ty, span));
                    s.emit_value(InstKind::ListTruncate(l, last), list_ty, span)
                });
                let some = self.make_variant(option_ty, 0, &[popped.expect("set")], span);
                self.emit(InstKind::Store(result, some), None, span);
                self.terminate(Terminator::Jump(join));

                self.switch_to(missing);
                let none = self.make_variant(option_ty, 1, &[], span);
                self.emit(InstKind::Store(result, none), None, span);
                self.terminate(Terminator::Jump(join));

                self.switch_to(join);
                Some(self.emit_value(InstKind::Load(result), result_ir, span))
            }
            _ => unreachable!("only changing operations have a place receiver"),
        }
    }

    fn field_type(&self, ty: IrType, index: usize) -> IrType {
        let IrType::Struct(id) = ty else {
            unreachable!("checked: only structs have fields");
        };
        self.m.module.structs[id.index()].fields[index].ty
    }

    /// Lowers `stmts` into the current block and, if control can still fall
    /// through at the end, jumps to `target`. Returns whether it jumped.
    fn stmts_then_jump(&mut self, stmts: &[CheckedStmt], target: BlockId) -> bool {
        self.stmts(stmts);
        if self.terminated {
            return false;
        }
        self.terminate(Terminator::Jump(target));
        true
    }

    fn if_stmt(
        &mut self,
        cond: &CheckedExpr,
        then_body: &[CheckedStmt],
        else_body: &[CheckedStmt],
    ) {
        let c = self.value(cond);
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join = self.new_block();
        self.terminate(Terminator::Branch {
            cond: c,
            then_block,
            else_block,
        });

        self.switch_to(then_block);
        let then_falls_through = self.stmts_then_jump(then_body, join);
        self.switch_to(else_block);
        let else_falls_through = self.stmts_then_jump(else_body, join);

        self.switch_to(join);
        if !then_falls_through && !else_falls_through {
            // Both branches left the block: the join block has no
            // predecessors, and anything after the `if` is unreachable.
            self.terminated = true;
        }
    }

    fn while_stmt(&mut self, cond: &CheckedExpr, body: &[CheckedStmt]) {
        let header = self.new_block();
        let body_block = self.new_block();
        let exit = self.new_block();
        self.terminate(Terminator::Jump(header));

        self.switch_to(header);
        let c = self.value(cond);
        self.terminate(Terminator::Branch {
            cond: c,
            then_block: body_block,
            else_block: exit,
        });

        self.switch_to(body_block);
        self.loops.push((header, exit));
        self.stmts_then_jump(body, header);
        self.loops.pop();

        self.switch_to(exit);
    }

    /// The scrutinee is stored in a slot. Each arm tests it, binds its
    /// names and checks its guard, and falls through to the next arm when
    /// any of that fails.
    fn match_stmt(&mut self, scrutinee: &CheckedExpr, arms: &[CheckedArm]) {
        let span = scrutinee.span;
        let value = self.value(scrutinee);
        let ty = self.concrete(scrutinee.ty);
        let ir_ty = self.m.ir_type(ty);
        let slot = self.new_slot("match", ir_ty);
        self.emit(InstKind::Store(slot, value), None, span);

        let join = self.new_block();
        let mut falls_through = false;
        for arm in arms {
            let next = self.new_block();
            self.match_pattern(&arm.pattern, slot, ty, next, span);
            if let Some(guard) = &arm.guard {
                let cond = self.value(guard);
                let body = self.new_block();
                self.terminate(Terminator::Branch {
                    cond,
                    then_block: body,
                    else_block: next,
                });
                self.switch_to(body);
            }
            falls_through |= self.stmts_then_jump(&arm.body, join);
            self.switch_to(next);
        }
        // The arms are exhaustive: no value gets here.
        self.terminate(Terminator::Unreachable);

        self.switch_to(join);
        if !falls_through {
            self.terminated = true;
        }
    }

    /// Tests the value in `slot`, of type `ty`, against `pattern` and
    /// stores the parts it binds. Continues in a new block when it matches;
    /// jumps to `fail` when it does not.
    fn match_pattern(
        &mut self,
        pattern: &CheckedPattern,
        slot: SlotId,
        ty: Type,
        fail: BlockId,
        span: Span,
    ) {
        let ir_ty = self.func.slots[slot.index()].ty;
        let cond = match pattern {
            CheckedPattern::Wildcard => return,
            CheckedPattern::Binding(local) => {
                let v = self.emit_value(InstKind::Load(slot), ir_ty, span);
                self.emit(InstKind::Store(SlotId(local.0 as u32), v), None, span);
                return;
            }
            CheckedPattern::Or(alternatives) => {
                let matched = self.new_block();
                let (last, rest) = alternatives.split_last().expect("at least two");
                for alternative in rest {
                    let other = self.new_block();
                    self.match_pattern(alternative, slot, ty, other, span);
                    self.terminate(Terminator::Jump(matched));
                    self.switch_to(other);
                }
                self.match_pattern(last, slot, ty, fail, span);
                self.terminate(Terminator::Jump(matched));
                self.switch_to(matched);
                return;
            }
            CheckedPattern::Variant { index, .. } => {
                let v = self.emit_value(InstKind::Load(slot), ir_ty, span);
                let tag = self.emit_value(InstKind::Extract(v, 0), IrType::I64, span);
                let expected = self.emit_value(
                    InstKind::Const(Const::I64(*index as i64)),
                    IrType::I64,
                    span,
                );
                self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::IEq, tag, expected),
                    IrType::Bool,
                    span,
                )
            }
            CheckedPattern::Int(n) => {
                let v = self.emit_value(InstKind::Load(slot), IrType::I64, span);
                let c = self.emit_value(InstKind::Const(Const::I64(*n)), IrType::I64, span);
                self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::IEq, v, c),
                    IrType::Bool,
                    span,
                )
            }
            CheckedPattern::Bool(b) => {
                let v = self.emit_value(InstKind::Load(slot), IrType::Bool, span);
                let c = self.emit_value(InstKind::Const(Const::Bool(*b)), IrType::Bool, span);
                self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::BEq, v, c),
                    IrType::Bool,
                    span,
                )
            }
            CheckedPattern::Str(s) => {
                let v = self.emit_value(InstKind::Load(slot), IrType::Str, span);
                let c = self.emit_value(InstKind::Const(Const::Str(s.clone())), IrType::Str, span);
                self.emit_value(
                    InstKind::CallRuntime(RuntimeFn::StrEq, vec![v, c]),
                    IrType::Bool,
                    span,
                )
            }
        };
        let matched = self.new_block();
        self.terminate(Terminator::Branch {
            cond,
            then_block: matched,
            else_block: fail,
        });
        self.switch_to(matched);

        let CheckedPattern::Variant { index, fields } = pattern else {
            return;
        };
        let offset = self.m.enum_layout(ty).offsets[*index];
        for (j, (field, field_ty)) in fields.iter().enumerate() {
            if matches!(field, CheckedPattern::Wildcard) {
                continue;
            }
            let field_ty = self.concrete(*field_ty);
            let field_ir_ty = self.m.ir_type(field_ty);
            let v = self.emit_value(InstKind::Load(slot), ir_ty, span);
            let x = self.emit_value(InstKind::Extract(v, offset + j), field_ir_ty, span);
            let field_slot = match field {
                CheckedPattern::Binding(local) => {
                    self.emit(InstKind::Store(SlotId(local.0 as u32), x), None, span);
                    continue;
                }
                _ => self.new_slot("match", field_ir_ty),
            };
            self.emit(InstKind::Store(field_slot, x), None, span);
            self.match_pattern(field, field_slot, field_ty, fail, span);
        }
    }

    // ---- expressions --------------------------------------------------

    /// Lowers an expression that must produce a value.
    fn value(&mut self, expr: &CheckedExpr) -> ValueId {
        self.expr(expr)
            .expect("semantic analysis rejects void values")
    }

    /// Lowers several operands, left to right, so that all their values are
    /// usable in the block that is current afterwards.
    ///
    /// IR values only live within their block, but an operand containing
    /// `&&` or `||` ends the current block. Values computed before such an
    /// operand are therefore stored in temporary slots and loaded again
    /// after it.
    fn values(&mut self, exprs: &[&CheckedExpr]) -> Vec<ValueId> {
        let mut values: Vec<ValueId> = Vec::with_capacity(exprs.len());
        for (i, expr) in exprs.iter().enumerate() {
            if i > 0 && branches(expr) {
                let spilled: Vec<(SlotId, IrType)> = values
                    .iter()
                    .zip(exprs)
                    .map(|(v, e)| {
                        let ty = self.ir_type(e.ty);
                        let slot = self.new_slot("tmp", ty);
                        self.emit(InstKind::Store(slot, *v), None, e.span);
                        (slot, ty)
                    })
                    .collect();
                let value = self.value(expr);
                values = spilled
                    .into_iter()
                    .zip(exprs)
                    .map(|((slot, ty), e)| self.emit_value(InstKind::Load(slot), ty, e.span))
                    .collect();
                values.push(value);
            } else {
                values.push(self.value(expr));
            }
        }
        values
    }

    /// Lowers an expression; `None` for calls to `void` functions.
    fn expr(&mut self, expr: &CheckedExpr) -> Option<ValueId> {
        let span = expr.span;
        let ty = self.ir_type(expr.ty);
        let value = match &expr.kind {
            CheckedExprKind::Int(v) => self.emit_value(InstKind::Const(Const::I64(*v)), ty, span),
            CheckedExprKind::Float(bits) => {
                self.emit_value(InstKind::Const(Const::F64(*bits)), ty, span)
            }
            CheckedExprKind::Bool(v) => self.emit_value(InstKind::Const(Const::Bool(*v)), ty, span),
            CheckedExprKind::Str(v) => {
                self.emit_value(InstKind::Const(Const::Str(v.clone())), ty, span)
            }
            CheckedExprKind::Local(local) => {
                self.emit_value(InstKind::Load(SlotId(local.0 as u32)), ty, span)
            }
            CheckedExprKind::Unary { op, operand } => {
                let v = self.value(operand);
                let op = match op {
                    UnaryOp::IntNeg => brim_ir::UnaryOp::INeg,
                    UnaryOp::FloatNeg => brim_ir::UnaryOp::FNeg,
                    UnaryOp::BoolNot => brim_ir::UnaryOp::BNot,
                };
                self.emit_value(InstKind::Unary(op, v), ty, span)
            }
            CheckedExprKind::Binary { op, lhs, rhs } => {
                let [a, b] = self.values(&[lhs, rhs]).try_into().expect("two operands");
                self.binary(*op, a, b, span)
            }
            CheckedExprKind::And(lhs, rhs) => self.short_circuit(lhs, rhs, true, span),
            CheckedExprKind::Or(lhs, rhs) => self.short_circuit(lhs, rhs, false, span),
            CheckedExprKind::Call {
                func,
                type_args,
                args,
            } => {
                let type_args: Vec<Type> = type_args.iter().map(|t| self.concrete(*t)).collect();
                let callee = self.m.instance(func.0, type_args);
                let args = self.values(&args.iter().collect::<Vec<_>>());
                return self.emit(InstKind::Call(callee, args), Some(ty), span);
            }
            CheckedExprKind::Builtin { runtime, args } => {
                let args = self.values(&args.iter().collect::<Vec<_>>());
                return self.emit(InstKind::CallRuntime(*runtime, args), Some(ty), span);
            }
            CheckedExprKind::CallMut {
                func,
                type_args,
                place,
                args,
            } => {
                let type_args: Vec<Type> = type_args.iter().map(|t| self.concrete(*t)).collect();
                let callee = self.m.instance(func.0, type_args);
                let self_ty = self.ir_type(place.ty);
                let pair = (ty != IrType::Void).then(|| self.m.pair_type(ty, self_ty));
                let indices = self.place_indices(place);
                let (indices, args) = if args.iter().any(branches) {
                    let spilled = self.spill(&indices, span);
                    let values = self.values(&args.iter().collect::<Vec<_>>());
                    (self.reload(&spilled, span), values)
                } else {
                    let values = self.values(&args.iter().collect::<Vec<_>>());
                    (indices, values)
                };
                let mut result = None;
                self.write_place(place, &indices, span, |s, current| {
                    let mut call_args = Vec::with_capacity(args.len() + 1);
                    call_args.push(current);
                    call_args.extend(&args);
                    let call_ty = pair.map_or(self_ty, IrType::Struct);
                    let returned =
                        s.emit_value(InstKind::CallMut(callee, call_args), call_ty, span);
                    match pair {
                        None => returned,
                        Some(_) => {
                            result = Some(s.emit_value(InstKind::Extract(returned, 0), ty, span));
                            s.emit_value(InstKind::Extract(returned, 1), self_ty, span)
                        }
                    }
                });
                return result;
            }
            CheckedExprKind::StructLit { fields } => {
                // Evaluate in source order, then put the values in field order.
                let exprs: Vec<&CheckedExpr> = fields.iter().map(|(_, e)| e).collect();
                let values = self.values(&exprs);
                let mut ordered: Vec<Option<ValueId>> = vec![None; fields.len()];
                for ((index, _), value) in fields.iter().zip(values) {
                    ordered[*index] = Some(value);
                }
                let ordered = ordered
                    .into_iter()
                    .map(|v| v.expect("checked: every field is given once"))
                    .collect();
                let IrType::Struct(id) = ty else {
                    unreachable!("checked: a struct literal has a struct type");
                };
                self.emit_value(InstKind::MakeStruct(id, ordered), ty, span)
            }
            CheckedExprKind::Variant { index, args } => {
                let values = self.values(&args.iter().collect::<Vec<_>>());
                let enum_ty = self.concrete(expr.ty);
                self.make_variant(enum_ty, *index, &values, span)
            }
            CheckedExprKind::Try { operand } => self.try_expr(operand, ty, span),
            CheckedExprKind::Intrinsic { op, args } => self.intrinsic(*op, args, ty, expr.ty, span),
            CheckedExprKind::Field { base, index } => {
                let v = self.value(base);
                self.emit_value(InstKind::Extract(v, *index), ty, span)
            }
            CheckedExprKind::ListLit(items) => {
                let values = self.values(&items.iter().collect::<Vec<_>>());
                let IrType::List(id) = ty else {
                    unreachable!("a list literal has a list type");
                };
                self.emit_value(InstKind::ListNew(id, values), ty, span)
            }
            CheckedExprKind::Index { base, index } => {
                let [l, i] = self
                    .values(&[base, index])
                    .try_into()
                    .expect("two operands");
                if matches!(self.func.values[l.index()], IrType::Map(_)) {
                    let at = self.emit_value(InstKind::MapIndex(l, i), IrType::I64, span);
                    self.emit_value(InstKind::MapValueAt(l, at), ty, span)
                } else {
                    self.emit_value(InstKind::ListGet(l, i), ty, span)
                }
            }
            CheckedExprKind::MapLit(entries) => {
                let mut exprs = Vec::with_capacity(entries.len() * 2);
                for (k, v) in entries {
                    exprs.push(k);
                    exprs.push(v);
                }
                let values = self.values(&exprs);
                let IrType::Map(id) = ty else {
                    unreachable!("a map literal has a map type");
                };
                let mut map = self.emit_value(InstKind::MapNew(id), ty, span);
                for pair in values.chunks(2) {
                    map = self.emit_value(InstKind::MapInsert(map, pair[0], pair[1]), ty, span);
                }
                map
            }
            CheckedExprKind::MapOp { op, receiver, args } => {
                return self.map_op(*op, receiver, args, ty, expr.ty, span);
            }
            CheckedExprKind::ListOp { op, receiver, args } => {
                return self.list_op(*op, receiver, args, ty, expr.ty, span);
            }
        };
        Some(value)
    }

    // ---- intrinsics ----------------------------------------------------------

    fn runtime(&mut self, rt: RuntimeFn, args: Vec<ValueId>, ty: IrType, span: Span) -> ValueId {
        self.emit_value(InstKind::CallRuntime(rt, args), ty, span)
    }

    fn intrinsic(
        &mut self,
        op: Intrinsic,
        args: &[CheckedExpr],
        ty: IrType,
        result_ty: brim_semantic::Type,
        span: Span,
    ) -> ValueId {
        let args = self.values(&args.iter().collect::<Vec<_>>());
        let (i64_ty, bool_ty, str_ty) = (IrType::I64, IrType::Bool, IrType::Str);
        let enum_ty = self.concrete(result_ty);
        match op {
            Intrinsic::StrIsEmpty => {
                let len = self.runtime(RuntimeFn::StrLen, vec![args[0]], i64_ty, span);
                let zero = self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span);
                self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::IEq, len, zero),
                    bool_ty,
                    span,
                )
            }
            Intrinsic::StrFind => {
                let at = self.runtime(RuntimeFn::StrFind, args, i64_ty, span);
                let zero = self.emit_value(InstKind::Const(Const::I64(0)), i64_ty, span);
                let found = self.emit_value(
                    InstKind::Binary(brim_ir::BinaryOp::IGe, at, zero),
                    bool_ty,
                    span,
                );
                self.choose_variant(found, enum_ty, at, None, span)
            }
            Intrinsic::ParseInt => {
                let ok = self.runtime(RuntimeFn::StrParseIntOk, vec![args[0]], bool_ty, span);
                let value = self.runtime(RuntimeFn::StrParseInt, vec![args[0]], i64_ty, span);
                self.choose_variant(ok, enum_ty, value, None, span)
            }
            Intrinsic::ParseFloat => {
                let ok = self.runtime(RuntimeFn::StrParseFloatOk, vec![args[0]], bool_ty, span);
                let value =
                    self.runtime(RuntimeFn::StrParseFloat, vec![args[0]], IrType::F64, span);
                self.choose_variant(ok, enum_ty, value, None, span)
            }
            Intrinsic::Env => {
                let set = self.runtime(RuntimeFn::EnvHas, vec![args[0]], bool_ty, span);
                let value = self.runtime(RuntimeFn::EnvGet, vec![args[0]], str_ty, span);
                self.choose_variant(set, enum_ty, value, None, span)
            }
            Intrinsic::ReadLine | Intrinsic::ReadFile | Intrinsic::WriteFile => {
                let (rt, value_ty) = match op {
                    Intrinsic::ReadLine => (RuntimeFn::ReadLine, str_ty),
                    Intrinsic::ReadFile => (RuntimeFn::ReadFile, str_ty),
                    _ => (RuntimeFn::WriteFile, i64_ty),
                };
                let value = self.runtime(rt, args, value_ty, span);
                let error = self.runtime(RuntimeFn::LastError, vec![], str_ty, span);
                let empty =
                    self.emit_value(InstKind::Const(Const::Str(String::new())), str_ty, span);
                let ok = self.runtime(RuntimeFn::StrEq, vec![error, empty], bool_ty, span);
                // `read_line` gives `Option`; the file functions `Result`.
                let error = (op != Intrinsic::ReadLine).then_some(error);
                self.choose_variant(ok, enum_ty, value, error, span)
            }
            Intrinsic::Args => self.runtime(RuntimeFn::Args, vec![], ty, span),
        }
    }

    /// `Some(value)` / `Ok(value)` if `cond`, else `None` (no `error`) or
    /// `Err(error)`.
    fn choose_variant(
        &mut self,
        cond: ValueId,
        enum_ty: brim_semantic::Type,
        value: ValueId,
        error: Option<ValueId>,
        span: Span,
    ) -> ValueId {
        let value_ty = self.func.values[value.index()];
        let value_slot = self.new_slot("value", value_ty);
        self.emit(InstKind::Store(value_slot, value), None, span);
        let error_slot = error.map(|e| {
            let slot = self.new_slot("error", IrType::Str);
            self.emit(InstKind::Store(slot, e), None, span);
            slot
        });
        let result_ir = self.m.ir_type(enum_ty);
        let result = self.new_slot("result", result_ir);
        let yes = self.new_block();
        let no = self.new_block();
        let join = self.new_block();
        self.terminate(Terminator::Branch {
            cond,
            then_block: yes,
            else_block: no,
        });
        self.switch_to(yes);
        let value = self.emit_value(InstKind::Load(value_slot), value_ty, span);
        let good = self.make_variant(enum_ty, 0, &[value], span);
        self.emit(InstKind::Store(result, good), None, span);
        self.terminate(Terminator::Jump(join));
        self.switch_to(no);
        let payload: Vec<ValueId> = error_slot
            .map(|slot| self.emit_value(InstKind::Load(slot), IrType::Str, span))
            .into_iter()
            .collect();
        let bad = self.make_variant(enum_ty, 1, &payload, span);
        self.emit(InstKind::Store(result, bad), None, span);
        self.terminate(Terminator::Jump(join));
        self.switch_to(join);
        self.emit_value(InstKind::Load(result), result_ir, span)
    }

    /// Variant `index` of the concrete enum `enum_ty`, with `payload`.
    fn make_variant(
        &mut self,
        enum_ty: Type,
        index: usize,
        payload: &[ValueId],
        span: Span,
    ) -> ValueId {
        let layout = self.m.enum_layout(enum_ty);
        let ty = self.m.ir_type(enum_ty);
        let IrType::Struct(id) = ty else {
            unreachable!("enums lower to structs");
        };
        let field_types: Vec<IrType> = self.m.module.structs[id.index()]
            .fields
            .iter()
            .map(|f| f.ty)
            .collect();
        let start = layout.offsets[index];
        let mut fields = Vec::with_capacity(field_types.len());
        fields.push(self.emit_value(InstKind::Const(Const::I64(index as i64)), IrType::I64, span));
        for (i, field_ty) in field_types.iter().enumerate().skip(1) {
            let value = if i >= start && i < start + payload.len() {
                payload[i - start]
            } else {
                self.zero(*field_ty, span)
            };
            fields.push(value);
        }
        self.emit_value(InstKind::MakeStruct(id, fields), ty, span)
    }

    /// `operand?`: continues with the payload of variant 0 (`Some`/`Ok`), or
    /// returns variant 1 (`None`/`Err`, with the error value) from the
    /// function.
    fn try_expr(&mut self, operand: &CheckedExpr, ty: IrType, span: Span) -> ValueId {
        let value = self.value(operand);
        let operand_ty = self.concrete(operand.ty);
        let ir_ty = self.m.ir_type(operand_ty);
        let layout = self.m.enum_layout(operand_ty);
        let slot = self.new_slot("try", ir_ty);
        self.emit(InstKind::Store(slot, value), None, span);
        let tag = self.emit_value(InstKind::Extract(value, 0), IrType::I64, span);
        let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
        let has_value = self.emit_value(
            InstKind::Binary(brim_ir::BinaryOp::IEq, tag, zero),
            IrType::Bool,
            span,
        );
        let ok = self.new_block();
        let fail = self.new_block();
        self.terminate(Terminator::Branch {
            cond: has_value,
            then_block: ok,
            else_block: fail,
        });

        self.switch_to(fail);
        let failed = self.emit_value(InstKind::Load(slot), ir_ty, span);
        let error_fields = layout.offsets.get(2).copied().unwrap_or_else(|| {
            let IrType::Struct(id) = ir_ty else {
                unreachable!("enums lower to structs");
            };
            self.m.module.structs[id.index()].fields.len()
        }) - layout.offsets[1];
        let payload: Vec<ValueId> = (0..error_fields)
            .map(|j| {
                let field_ty = self.field_type(ir_ty, layout.offsets[1] + j);
                self.emit_value(
                    InstKind::Extract(failed, layout.offsets[1] + j),
                    field_ty,
                    span,
                )
            })
            .collect();
        let ret = self.ret;
        let returned = self.make_variant(ret, 1, &payload, span);
        // Through `return_value`: a `mut self` method also returns `self`.
        self.return_value(Some(returned), span);

        self.switch_to(ok);
        let loaded = self.emit_value(InstKind::Load(slot), ir_ty, span);
        self.emit_value(InstKind::Extract(loaded, layout.offsets[0]), ty, span)
    }

    fn binary(&mut self, op: BinaryOp, a: ValueId, b: ValueId, span: Span) -> ValueId {
        use brim_ir::BinaryOp as Ir;
        let ir_op = match op {
            BinaryOp::IntAdd => Ir::IAdd,
            BinaryOp::IntSub => Ir::ISub,
            BinaryOp::IntMul => Ir::IMul,
            BinaryOp::IntDiv => Ir::IDiv,
            BinaryOp::IntRem => Ir::IRem,
            BinaryOp::IntEq => Ir::IEq,
            BinaryOp::IntNe => Ir::INe,
            BinaryOp::IntLt => Ir::ILt,
            BinaryOp::IntLe => Ir::ILe,
            BinaryOp::IntGt => Ir::IGt,
            BinaryOp::IntGe => Ir::IGe,
            BinaryOp::BoolEq => Ir::BEq,
            BinaryOp::BoolNe => Ir::BNe,
            BinaryOp::FloatAdd => Ir::FAdd,
            BinaryOp::FloatSub => Ir::FSub,
            BinaryOp::FloatMul => Ir::FMul,
            BinaryOp::FloatDiv => Ir::FDiv,
            BinaryOp::FloatRem => Ir::FRem,
            BinaryOp::FloatEq => Ir::FEq,
            BinaryOp::FloatNe => Ir::FNe,
            BinaryOp::FloatLt => Ir::FLt,
            BinaryOp::FloatLe => Ir::FLe,
            BinaryOp::FloatGt => Ir::FGt,
            BinaryOp::FloatGe => Ir::FGe,
            BinaryOp::StrConcat => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrConcat, vec![a, b]);
                return self.emit_value(kind, IrType::Str, span);
            }
            BinaryOp::StrLt | BinaryOp::StrLe | BinaryOp::StrGt | BinaryOp::StrGe => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrCompare, vec![a, b]);
                let order = self.emit_value(kind, IrType::I64, span);
                let zero = self.emit_value(InstKind::Const(Const::I64(0)), IrType::I64, span);
                let cmp = match op {
                    BinaryOp::StrLt => Ir::ILt,
                    BinaryOp::StrLe => Ir::ILe,
                    BinaryOp::StrGt => Ir::IGt,
                    _ => Ir::IGe,
                };
                return self.emit_value(InstKind::Binary(cmp, order, zero), IrType::Bool, span);
            }
            BinaryOp::StrEq | BinaryOp::StrNe => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrEq, vec![a, b]);
                let eq = self.emit_value(kind, IrType::Bool, span);
                if op == BinaryOp::StrEq {
                    return eq;
                }
                return self.emit_value(
                    InstKind::Unary(brim_ir::UnaryOp::BNot, eq),
                    IrType::Bool,
                    span,
                );
            }
        };
        let ty = ir_op.signature().1;
        self.emit_value(InstKind::Binary(ir_op, a, b), ty, span)
    }

    /// `lhs && rhs` (`is_and`) or `lhs || rhs`: `rhs` is only evaluated when
    /// `lhs` does not already decide the result.
    fn short_circuit(
        &mut self,
        lhs: &CheckedExpr,
        rhs: &CheckedExpr,
        is_and: bool,
        span: Span,
    ) -> ValueId {
        let result = self.new_slot(if is_and { "and" } else { "or" }, IrType::Bool);
        let l = self.value(lhs);
        self.emit(InstKind::Store(result, l), None, lhs.span);
        let eval_rhs = self.new_block();
        let join = self.new_block();
        let (then_block, else_block) = if is_and {
            (eval_rhs, join)
        } else {
            (join, eval_rhs)
        };
        self.terminate(Terminator::Branch {
            cond: l,
            then_block,
            else_block,
        });

        self.switch_to(eval_rhs);
        let r = self.value(rhs);
        self.emit(InstKind::Store(result, r), None, rhs.span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(InstKind::Load(result), IrType::Bool, span)
    }
}

impl ModuleLowerer<'_> {
    /// A struct holding a `mut self` method's result and its new `self`.
    fn pair_type(&mut self, result: IrType, self_ty: IrType) -> StructId {
        if let Some(id) = self.pairs.get(&(result, self_ty)) {
            return *id;
        }
        let id =
            StructId(u32::try_from(self.module.structs.len()).expect("struct count fits in u32"));
        self.module.structs.push(StructDef {
            name: "(result, self)".into(),
            fields: vec![
                Field {
                    name: "result".into(),
                    ty: result,
                },
                Field {
                    name: "self".into(),
                    ty: self_ty,
                },
            ],
        });
        self.pairs.insert((result, self_ty), id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower_src(src: &str) -> Module {
        let (ast, diags) = brim_parser::parse(src);
        assert!(diags.is_empty(), "{diags:#?}");
        let (checked, diags) = brim_semantic::check(&ast);
        let checked = checked.unwrap_or_else(|| panic!("{diags:#?}"));
        let module = lower(&checked);
        if let Err(errors) = brim_ir::validate(&module) {
            panic!("invalid IR:\n{module}\n{errors:#?}");
        }
        module
    }

    #[test]
    fn lowers_functions_and_calls() {
        let module = lower_src(
            "fn add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfn main() {\n    let result: i64 = add(10, 32)\n    print(result)\n}\n",
        );
        assert_eq!(
            module.to_string(),
            "; entry: main

fn main() {
  slot s0 result: i64
bb0:
  v0 = const i64 10
  v1 = const i64 32
  v2 = call add(v0, v1)
  store s0, v2
  v3 = load s0
  call @brim_rt_print_i64(v3)
  ret
}

fn add(s0 a: i64, s1 b: i64) -> i64 {
bb0:
  v0 = load s0
  v1 = load s1
  v2 = iadd v0, v1
  ret v2
}
"
        );
    }

    #[test]
    fn lowers_short_circuit_operators() {
        let module = lower_src("fn main() {\n    print(true && false || true)\n}");
        let main = &module.functions[0];
        // `&&` and `||` each add a right-hand-side block and a join block.
        assert_eq!(main.blocks.len(), 5);
        assert_eq!(main.slots.len(), 2);
    }

    #[test]
    fn lowers_string_operations_to_runtime_calls() {
        let module = lower_src(
            "fn main() {\n    let s: string = \"a\" + \"b\"\n    print(s == \"ab\")\n    print(s != \"ab\")\n}",
        );
        let text = module.to_string();
        assert!(text.contains("call @brim_rt_str_concat(v0, v1)"));
        assert_eq!(text.matches("call @brim_rt_str_eq").count(), 2);
        assert!(text.contains("bnot"));
    }

    #[test]
    fn skips_unreachable_code() {
        let module = lower_src("fn f() -> i64 {\n    return 1\n    print(2)\n}\nfn main() {}");
        assert!(!module.to_string().contains("print"));
    }

    #[test]
    fn nested_blocks_and_shadowing_use_distinct_slots() {
        let module = lower_src(
            "fn main() {\n    let x: i64 = 1\n    {\n        let x: string = \"s\"\n        print(x)\n    }\n    print(x)\n}",
        );
        let main = &module.functions[0];
        assert_eq!(main.slots.len(), 2);
        assert_eq!(main.slots[0].ty, IrType::I64);
        assert_eq!(main.slots[1].ty, IrType::Str);
    }

    #[test]
    fn lowers_while_loops() {
        let module = lower_src(
            "fn main() {\n    let mut i: i64 = 0\n    while i < 3 {\n        i += 1\n    }\n    print(i)\n}",
        );
        assert_eq!(
            module.to_string(),
            "; entry: main

fn main() {
  slot s0 i: i64
bb0:
  v0 = const i64 0
  store s0, v0
  jump bb1
bb1:
  v1 = load s0
  v2 = const i64 3
  v3 = ilt v1, v2
  branch v3, bb2, bb3
bb2:
  v4 = load s0
  v5 = const i64 1
  v6 = iadd v4, v5
  store s0, v6
  jump bb1
bb3:
  v7 = load s0
  call @brim_rt_print_i64(v7)
  ret
}
"
        );
    }

    #[test]
    fn lowers_only_reachable_functions() {
        let module = lower_src(
            "fn used() -> i64 {\n    return 1\n}\nfn unused() -> i64 {\n    return 2\n}\nfn main() {\n    print(used())\n}",
        );
        let names: Vec<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["main", "used"]);
    }

    #[test]
    fn lowers_branches_that_return() {
        // Both branches return, so the join block is left without
        // predecessors and nothing follows it.
        let module = lower_src(
            "fn sign(n: i64) -> i64 {\n    if n < 0 {\n        return -1\n    } else if n == 0 {\n        return 0\n    } else {\n        return 1\n    }\n}\nfn main() {\n    print(sign(-5))\n}",
        );
        let sign = module
            .functions
            .iter()
            .find(|f| f.name == "sign")
            .expect("sign is called, so it is lowered");
        assert!(matches!(
            sign.blocks.last().unwrap().term,
            Terminator::Unreachable
        ));
    }

    #[test]
    fn lowers_break_and_continue() {
        let module = lower_src(
            "fn main() {\n    let mut i: i64 = 0\n    while true {\n        i += 1\n        if i == 2 {\n            continue\n        }\n        if i > 4 {\n            break\n        }\n        print(i)\n    }\n}",
        );
        let text = module.to_string();
        assert!(text.contains("jump bb1"), "{text}");
        assert!(text.contains("jump bb3"), "{text}");
    }

    #[test]
    fn monomorphizes_generic_functions() {
        let module = lower_src(
            "fn id[T](x: T) -> T {\n    return x\n}\nfn main() {\n    print(id(1))\n    print(id(\"a\"))\n    print(id(2))\n}",
        );
        let names: Vec<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
        // Non-generic functions first, then one instance per type argument.
        assert_eq!(names, ["main", "id[i64]", "id[string]"]);
    }

    #[test]
    fn lowers_enums_to_tagged_structs() {
        let module = lower_src(
            "enum E {\n    A(i64)\n    B(string, bool)\n    C\n}\nfn main() {\n    let e: E = E.B(\"x\", true)\n    match e {\n        A(n) => print(n)\n        _ => {}\n    }\n}",
        );
        assert!(
            module
                .to_string()
                .contains("struct %0 E { tag: i64, A.0: i64, B.0: str, B.1: bool }"),
            "{module}"
        );
    }
}
