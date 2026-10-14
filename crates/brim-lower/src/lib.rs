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
//! Every concrete struct type becomes an IR struct. An enum becomes an IR
//! struct too: field 0 is the variant's index (the *tag*), followed by the
//! payload fields of every variant in turn. A value only uses its own
//! variant's fields; the others hold zero values. `match` loads the tag and
//! branches on it.

use std::collections::{HashMap, VecDeque};

use brim_ir::{
    Block, BlockId, Const, Field, FuncId, Function, Inst, InstKind, Module, Slot, SlotId, Span,
    StructDef, StructId, Terminator, Type as IrType, ValueId,
};
use brim_runtime::RuntimeFn;
use brim_semantic::checked::{
    BinaryOp, CheckedArm, CheckedExpr, CheckedExprKind, CheckedFn, CheckedPattern, CheckedProgram,
    CheckedStmt, UnaryOp,
};
use brim_semantic::{Def, Type, TypeTable};

/// Lowers a checked program to IR.
pub fn lower(program: &CheckedProgram) -> Module {
    let mut m = ModuleLowerer {
        program,
        types: program.types.clone(),
        module: Module {
            structs: Vec::new(),
            functions: Vec::new(),
            entry: FuncId(0),
        },
        ir_types: HashMap::new(),
        enum_layouts: HashMap::new(),
        instances: HashMap::new(),
        queue: VecDeque::new(),
    };
    for (index, f) in program.functions.iter().enumerate() {
        if f.type_params.is_empty() {
            m.instance(index, Vec::new());
        }
    }
    m.module.entry = m.instance(program.entry.0, Vec::new());
    while let Some((id, index, type_args)) = m.queue.pop_front() {
        let mut function = FnLowerer::lower(&mut m, &program.functions[index], type_args);
        function.name = std::mem::take(&mut m.module.functions[id.index()].name);
        m.module.functions[id.index()] = function;
    }
    m.module
}

fn func_id(index: usize) -> FuncId {
    FuncId(u32::try_from(index).expect("function count fits in u32"))
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
                };
                if named.args.is_empty() {
                    name.clone()
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
            Type::Bool => IrType::Bool,
            Type::String => IrType::Str,
            Type::Void => IrType::Void,
            Type::Named(id) => {
                if let Some(ir) = self.ir_types.get(&ty) {
                    return *ir;
                }
                let named = self.types.get(id).clone();
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
                }
                let id = StructId(
                    u32::try_from(self.module.structs.len()).expect("struct count fits in u32"),
                );
                self.module.structs.push(StructDef {
                    name: self.type_name(ty),
                    fields,
                });
                let ir = IrType::Struct(id);
                self.ir_types.insert(ty, ir);
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
        | CheckedExprKind::Bool(_)
        | CheckedExprKind::Str(_)
        | CheckedExprKind::Local(_) => false,
        CheckedExprKind::Unary { operand, .. } => branches(operand),
        CheckedExprKind::Binary { lhs, rhs, .. } => branches(lhs) || branches(rhs),
        CheckedExprKind::Call { args, .. }
        | CheckedExprKind::Builtin { args, .. }
        | CheckedExprKind::Variant { args, .. } => args.iter().any(branches),
        CheckedExprKind::StructLit { fields } => fields.iter().any(|(_, v)| branches(v)),
        CheckedExprKind::Field { base, .. } => branches(base),
    }
}

struct FnLowerer<'m, 'p> {
    m: &'m mut ModuleLowerer<'p>,
    /// The type arguments of this instance.
    type_args: Vec<Type>,
    /// The function's concrete return type.
    ret: Type,
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
            func: Function {
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
        lowerer.current = lowerer.new_block();
        lowerer.stmts(&f.body);
        if !lowerer.terminated {
            // Semantic analysis guarantees non-void functions always return.
            let term = if f.ret == Type::Void {
                Terminator::Return(None)
            } else {
                Terminator::Unreachable
            };
            lowerer.terminate(term);
        }
        lowerer.func
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
            }
            CheckedStmt::Return { value, .. } => {
                let value = value.as_ref().map(|v| self.value(v));
                self.terminate(Terminator::Return(value));
            }
            CheckedStmt::Block(stmts) => self.stmts(stmts),
            CheckedStmt::Assign { local, path, value } => {
                let v = self.value(value);
                let slot = SlotId(local.0 as u32);
                let v = if path.is_empty() {
                    v
                } else {
                    self.update_path(slot, path, v, value.span)
                };
                self.emit(InstKind::Store(slot, v), None, value.span);
            }
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

    /// For `slot.a.b = v`: the slot's new struct value, with field `a.b`
    /// replaced by `v`.
    fn update_path(&mut self, slot: SlotId, path: &[usize], v: ValueId, span: Span) -> ValueId {
        let root_ty = self.func.slots[slot.index()].ty;
        // levels[i] is the struct that contains field path[i].
        let mut levels = vec![(
            self.emit_value(InstKind::Load(slot), root_ty, span),
            root_ty,
        )];
        for &index in &path[..path.len() - 1] {
            let (value, ty) = *levels.last().expect("not empty");
            let field_ty = self.field_type(ty, index);
            let field = self.emit_value(InstKind::Extract(value, index), field_ty, span);
            levels.push((field, field_ty));
        }
        let mut new = v;
        for (&index, (value, ty)) in path.iter().zip(levels).rev() {
            new = self.emit_value(InstKind::Insert(value, index, new), ty, span);
        }
        new
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

    /// The scrutinee is stored in a slot; each arm tests it in its own block
    /// and falls through to the next arm's test when it does not match.
    fn match_stmt(&mut self, scrutinee: &CheckedExpr, arms: &[CheckedArm]) {
        let span = scrutinee.span;
        let value = self.value(scrutinee);
        let ty = self.concrete(scrutinee.ty);
        let ir_ty = self.m.ir_type(ty);
        let slot = self.new_slot("match", ir_ty);
        self.emit(InstKind::Store(slot, value), None, span);
        let layout = match self.m.types_def(ty) {
            Some(Def::Enum(_)) => Some(self.m.enum_layout(ty)),
            _ => None,
        };

        let join = self.new_block();
        let mut falls_through = false;
        for arm in arms {
            let body = self.new_block();
            let next = self.new_block();
            let cond = match &arm.pattern {
                CheckedPattern::Wildcard => None,
                CheckedPattern::Variant { index, .. } => {
                    let v = self.emit_value(InstKind::Load(slot), ir_ty, span);
                    let tag = self.emit_value(InstKind::Extract(v, 0), IrType::I64, span);
                    let expected = self.emit_value(
                        InstKind::Const(Const::I64(*index as i64)),
                        IrType::I64,
                        span,
                    );
                    Some(self.emit_value(
                        InstKind::Binary(brim_ir::BinaryOp::IEq, tag, expected),
                        IrType::Bool,
                        span,
                    ))
                }
                CheckedPattern::Int(n) => {
                    let v = self.emit_value(InstKind::Load(slot), IrType::I64, span);
                    let c = self.emit_value(InstKind::Const(Const::I64(*n)), IrType::I64, span);
                    Some(self.emit_value(
                        InstKind::Binary(brim_ir::BinaryOp::IEq, v, c),
                        IrType::Bool,
                        span,
                    ))
                }
                CheckedPattern::Bool(b) => {
                    let v = self.emit_value(InstKind::Load(slot), IrType::Bool, span);
                    let c = self.emit_value(InstKind::Const(Const::Bool(*b)), IrType::Bool, span);
                    Some(self.emit_value(
                        InstKind::Binary(brim_ir::BinaryOp::BEq, v, c),
                        IrType::Bool,
                        span,
                    ))
                }
                CheckedPattern::Str(s) => {
                    let v = self.emit_value(InstKind::Load(slot), IrType::Str, span);
                    let c =
                        self.emit_value(InstKind::Const(Const::Str(s.clone())), IrType::Str, span);
                    Some(self.emit_value(
                        InstKind::CallRuntime(RuntimeFn::StrEq, vec![v, c]),
                        IrType::Bool,
                        span,
                    ))
                }
            };
            match cond {
                Some(cond) => self.terminate(Terminator::Branch {
                    cond,
                    then_block: body,
                    else_block: next,
                }),
                None => self.terminate(Terminator::Jump(body)),
            }

            self.switch_to(body);
            if let CheckedPattern::Variant { index, bindings } = &arm.pattern {
                let offset = layout
                    .as_ref()
                    .expect("variant patterns match enums")
                    .offsets[*index];
                let v = self.emit_value(InstKind::Load(slot), ir_ty, span);
                for (j, binding) in bindings.iter().enumerate() {
                    if let Some(local) = binding {
                        let local_slot = SlotId(local.0 as u32);
                        let field_ty = self.func.slots[local_slot.index()].ty;
                        let x = self.emit_value(InstKind::Extract(v, offset + j), field_ty, span);
                        self.emit(InstKind::Store(local_slot, x), None, span);
                    }
                }
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
            CheckedExprKind::Field { base, index } => {
                let v = self.value(base);
                self.emit_value(InstKind::Extract(v, *index), ty, span)
            }
        };
        Some(value)
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
        self.terminate(Terminator::Return(Some(returned)));

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
            BinaryOp::StrConcat => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrConcat, vec![a, b]);
                return self.emit_value(kind, IrType::Str, span);
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
    /// The declaration a concrete type refers to, if it is named.
    fn types_def(&self, ty: Type) -> Option<Def> {
        match ty {
            Type::Named(id) => Some(self.types.get(id).def),
            _ => None,
        }
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

fn add(s0 a: i64, s1 b: i64) -> i64 {
bb0:
  v0 = load s0
  v1 = load s1
  v2 = iadd v0, v1
  ret v2
}

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
    fn lowers_branches_that_return() {
        // Both branches return, so the join block is left without
        // predecessors and nothing follows it.
        let module = lower_src(
            "fn sign(n: i64) -> i64 {\n    if n < 0 {\n        return -1\n    } else if n == 0 {\n        return 0\n    } else {\n        return 1\n    }\n}\nfn main() {\n    print(sign(-5))\n}",
        );
        let sign = &module.functions[0];
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
