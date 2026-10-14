//! Lowering of Brim IR to LLVM IR.
//!
//! The mapping is direct (see `docs/architecture.md`, "Code generation"):
//!
//! - Every IR function becomes an internal LLVM function named `brim.<name>`.
//!   The exported symbol `brim_entry` calls the entry function; the runtime's
//!   `main` calls `brim_entry`.
//! - Slots become `alloca`s in a dedicated entry block, which also stores the
//!   parameters and then jumps to IR block 0. Values become SSA values, and
//!   IR blocks map one to one onto LLVM blocks.
//! - `i64` is `i64`, `bool` is `i1` (widened to `i8` at runtime calls, as the
//!   runtime ABI specifies), `str` is a pointer to a `BrimStr` object.
//!   String literals are constant `BrimStr` objects with reference count -1.
//! - Checked arithmetic uses `llvm.s*.with.overflow` and explicit comparisons
//!   for division, branching to a block that calls `brim_rt_trap`.
//! - Structs become named LLVM struct types (`brim.<name>`), built and read
//!   with `insertvalue` / `extractvalue`.
//! - Strings are reference counted (`brim_rt_retain` / `brim_rt_release`),
//!   including strings inside structs: every slot owns the strings it
//!   holds, every non-constant value holding strings is owned by its block
//!   and released at the block's end, parameters are borrowed, and functions
//!   return values at +1.
//! - Every call to a Brim function is bracketed by `brim_rt_enter` and
//!   `brim_rt_leave`, which maintain the call stack shown in run-time errors
//!   and enforce the same call-depth limit as the interpreter.

use std::collections::HashMap;

use brim_diagnostics::{SourceFile, Span};
use brim_ir::{
    BinaryOp, Const, FuncId, InstKind, Module as IrModule, RuntimeFn, Terminator, Type, UnaryOp,
};
use brim_runtime::{AbiType, MAX_CALL_DEPTH, RuntimeError};
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::{Builder, BuilderError};
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, InitializationConfig, RelocMode, Target, TargetData, TargetMachine,
};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, StructType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::{AddressSpace, IntPredicate, OptimizationLevel};

use crate::trap::{self, TrapText};
use crate::{BuildOptions, BuildOutput, CodegenError, OptLevel};

pub(crate) fn build(
    module: &IrModule,
    options: &BuildOptions,
) -> Result<BuildOutput, CodegenError> {
    let machine = target_machine(options.opt_level)?;
    let context = Context::create();
    let llvm_module = lower(&context, module, &options.source, &machine)?;
    if options.opt_level == OptLevel::Release {
        llvm_module
            .run_passes("default<O2>", &machine, PassBuilderOptions::create())
            .map_err(|e| CodegenError::Llvm(e.to_string()))?;
    }
    crate::link::link(&machine, &llvm_module, options)
}

pub(crate) fn emit_ir(module: &IrModule, source: &SourceFile) -> Result<String, CodegenError> {
    let machine = target_machine(OptLevel::Debug)?;
    let context = Context::create();
    let llvm_module = lower(&context, module, source, &machine)?;
    Ok(llvm_module.print_to_string().to_string())
}

/// A target machine for the host. Executables use the generic CPU of the
/// host architecture, so they also run on other machines of that kind.
fn target_machine(opt_level: OptLevel) -> Result<TargetMachine, CodegenError> {
    Target::initialize_native(&InitializationConfig::default()).map_err(CodegenError::Llvm)?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| CodegenError::Llvm(e.to_string()))?;
    let level = match opt_level {
        OptLevel::Debug => OptimizationLevel::None,
        OptLevel::Release => OptimizationLevel::Default,
    };
    target
        .create_target_machine(
            &triple,
            "generic",
            "",
            level,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| CodegenError::Llvm(format!("cannot create a target machine for `{triple}`")))
}

fn lower<'ctx>(
    context: &'ctx Context,
    ir: &IrModule,
    source: &SourceFile,
    machine: &TargetMachine,
) -> Result<Module<'ctx>, CodegenError> {
    let module = context.create_module(source.name());
    module.set_triple(&machine.get_triple());
    module.set_data_layout(&TargetData::get_data_layout(&machine.get_target_data()));
    let mut lowerer = Lowerer::new(context, module, ir, source);
    lowerer
        .lower_module()
        .map_err(|e| CodegenError::Llvm(e.to_string()))?;
    let module = lowerer.module;
    module
        .verify()
        .map_err(|e| CodegenError::Llvm(e.to_string()))?;
    Ok(module)
}

struct Lowerer<'ctx, 'a> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    ir: &'a IrModule,
    source: &'a SourceFile,
    /// `brim.<name>` for every IR function, indexed by `FuncId`.
    functions: Vec<FunctionValue<'ctx>>,
    runtime: HashMap<RuntimeFn, FunctionValue<'ctx>>,
    /// `BrimStr` constants, by text.
    strings: HashMap<String, PointerValue<'ctx>>,
    /// NUL-terminated C strings, by text.
    c_strings: HashMap<String, PointerValue<'ctx>>,
    enter: FunctionValue<'ctx>,
    leave: FunctionValue<'ctx>,
    trap: FunctionValue<'ctx>,
    retain: FunctionValue<'ctx>,
    release: FunctionValue<'ctx>,
    /// `brim.<name>` for every IR struct, indexed by `StructId`.
    struct_types: Vec<StructType<'ctx>>,
    /// Whether each struct contains strings, directly or in nested structs.
    managed_structs: Vec<bool>,
}

impl<'ctx, 'a> Lowerer<'ctx, 'a> {
    fn new(
        context: &'ctx Context,
        module: Module<'ctx>,
        ir: &'a IrModule,
        source: &'a SourceFile,
    ) -> Self {
        let ptr = context.ptr_type(AddressSpace::default());
        let void = context.void_type();
        let enter = module.add_function(
            "brim_rt_enter",
            void.fn_type(&[ptr.into(), ptr.into(), ptr.into(), ptr.into()], false),
            Some(Linkage::External),
        );
        let leave = module.add_function(
            "brim_rt_leave",
            void.fn_type(&[], false),
            Some(Linkage::External),
        );
        let trap = module.add_function(
            "brim_rt_trap",
            void.fn_type(&[ptr.into(), ptr.into(), ptr.into()], false),
            Some(Linkage::External),
        );
        let retain = module.add_function(
            "brim_rt_retain",
            void.fn_type(&[ptr.into()], false),
            Some(Linkage::External),
        );
        let release = module.add_function(
            "brim_rt_release",
            void.fn_type(&[ptr.into()], false),
            Some(Linkage::External),
        );
        for attr in ["noreturn", "cold", "nounwind"] {
            let kind = Attribute::get_named_enum_kind_id(attr);
            trap.add_attribute(
                AttributeLoc::Function,
                context.create_enum_attribute(kind, 0),
            );
        }
        Lowerer {
            context,
            module,
            builder: context.create_builder(),
            ir,
            source,
            functions: Vec::new(),
            runtime: HashMap::new(),
            strings: HashMap::new(),
            c_strings: HashMap::new(),
            enter,
            leave,
            trap,
            retain,
            release,
            struct_types: Vec::new(),
            managed_structs: Vec::new(),
        }
    }

    fn lower_module(&mut self) -> Result<(), BuilderError> {
        // Named types first, so fields can refer to any struct.
        for def in &self.ir.structs {
            let ty = self
                .context
                .opaque_struct_type(&format!("brim.{}", def.name));
            self.struct_types.push(ty);
        }
        for (index, def) in self.ir.structs.iter().enumerate() {
            let fields: Vec<BasicTypeEnum<'ctx>> =
                def.fields.iter().map(|f| self.basic_type(f.ty)).collect();
            self.struct_types[index].set_body(&fields, false);
        }
        // Structs never contain themselves, so this fixpoint terminates.
        self.managed_structs = vec![false; self.ir.structs.len()];
        loop {
            let mut changed = false;
            for (index, def) in self.ir.structs.iter().enumerate() {
                let managed = def.fields.iter().any(|f| self.managed(f.ty));
                if managed && !self.managed_structs[index] {
                    self.managed_structs[index] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for f in &self.ir.functions {
            let params: Vec<Type> = f.param_types().collect();
            let fn_type = self.fn_type(f.ret, &params);
            let value = self.module.add_function(
                &format!("brim.{}", f.name),
                fn_type,
                Some(Linkage::Internal),
            );
            self.functions.push(value);
        }
        for index in 0..self.ir.functions.len() {
            self.lower_function(FuncId(index as u32))?;
        }
        self.lower_entry()
    }

    /// `brim_entry` and `brim_max_call_depth`, used by the runtime's `main`.
    fn lower_entry(&mut self) -> Result<(), BuilderError> {
        let i64_type = self.context.i64_type();
        let limit = self
            .module
            .add_global(i64_type, None, "brim_max_call_depth");
        limit.set_initializer(&i64_type.const_int(MAX_CALL_DEPTH as u64, false));
        limit.set_constant(true);

        let entry_fn = self.module.add_function(
            "brim_entry",
            self.context.void_type().fn_type(&[], false),
            Some(Linkage::External),
        );
        let block = self.context.append_basic_block(entry_fn, "entry");
        self.builder.position_at_end(block);
        let name = self.c_string(&self.ir.function(self.ir.entry).name.clone());
        // The first frame can never exceed the limit, so it needs no message.
        let null = self.context.ptr_type(AddressSpace::default()).const_null();
        self.builder.build_call(
            self.enter,
            &[name.into(), null.into(), null.into(), null.into()],
            "",
        )?;
        self.builder
            .build_call(self.functions[self.ir.entry.index()], &[], "")?;
        self.builder.build_call(self.leave, &[], "")?;
        self.builder.build_return(None)?;
        Ok(())
    }

    fn lower_function(&mut self, id: FuncId) -> Result<(), BuilderError> {
        let ir = self.ir;
        let f = ir.function(id);
        let function = self.functions[id.index()];

        let entry = self.context.append_basic_block(function, "entry");
        let blocks: Vec<BasicBlock<'ctx>> = (0..f.blocks.len())
            .map(|i| self.context.append_basic_block(function, &format!("bb{i}")))
            .collect();

        self.builder.position_at_end(entry);
        let mut slots = Vec::with_capacity(f.slots.len());
        for slot in &f.slots {
            slots.push(
                self.builder
                    .build_alloca(self.basic_type(slot.ty), &slot.name)?,
            );
        }
        // Slots holding strings own their references. Parameters are
        // borrowed from the caller, so the callee takes its own; other slots
        // start out empty (null strings, which retain and release ignore).
        for (index, slot) in f.slots.iter().enumerate() {
            match function.get_nth_param(index as u32) {
                Some(param) => {
                    self.builder.build_store(slots[index], param)?;
                    self.retain_value(param, slot.ty)?;
                }
                None if self.managed(slot.ty) => {
                    let zero = self.basic_type(slot.ty).const_zero();
                    self.builder.build_store(slots[index], zero)?;
                }
                None => {}
            }
        }
        self.builder.build_unconditional_branch(blocks[0])?;

        let managed_slots: Vec<(PointerValue<'ctx>, Type)> = slots
            .iter()
            .zip(&f.slots)
            .filter(|(_, slot)| self.managed(slot.ty))
            .map(|(ptr, slot)| (*ptr, slot.ty))
            .collect();
        let mut values: Vec<Option<BasicValueEnum<'ctx>>> = vec![None; f.values.len()];
        for (block, &llvm_block) in f.blocks.iter().zip(&blocks) {
            self.builder.position_at_end(llvm_block);
            // Values holding strings that this block owns a reference to,
            // released at its end. Values never outlive their block, so that
            // is always late enough.
            let mut owned: Vec<(BasicValueEnum<'ctx>, Type)> = Vec::new();
            for inst in &block.insts {
                let get = |v: brim_ir::ValueId| {
                    values[v.index()].expect("validated IR: values are defined before use")
                };
                let result: Option<BasicValueEnum<'ctx>> = match &inst.kind {
                    InstKind::Const(c) => Some(self.constant(c)),
                    InstKind::Load(slot) => {
                        let ty = self.basic_type(f.slots[slot.index()].ty);
                        Some(self.builder.build_load(ty, slots[slot.index()], "")?)
                    }
                    InstKind::Store(slot, v) => {
                        let ptr = slots[slot.index()];
                        let value = get(*v);
                        let ty = f.slots[slot.index()].ty;
                        if self.managed(ty) {
                            // Retain first: the old and new value may share
                            // strings.
                            self.retain_value(value, ty)?;
                            let old = self.builder.build_load(value.get_type(), ptr, "")?;
                            self.release_value(old, ty)?;
                        }
                        self.builder.build_store(ptr, value)?;
                        None
                    }
                    InstKind::Unary(op, v) => {
                        let v = get(*v).into_int_value();
                        Some(match op {
                            UnaryOp::INeg => {
                                let zero = self.context.i64_type().const_zero();
                                self.checked("llvm.ssub.with.overflow", zero, v, "-", inst.span)?
                            }
                            UnaryOp::BNot => self.builder.build_not(v, "")?.into(),
                        })
                    }
                    InstKind::Binary(op, a, b) => {
                        let (a, b) = (get(*a).into_int_value(), get(*b).into_int_value());
                        Some(self.binary(*op, a, b, inst.span)?)
                    }
                    InstKind::Call(callee, args) => {
                        let args: Vec<BasicMetadataValueEnum<'ctx>> =
                            args.iter().map(|a| get(*a).into()).collect();
                        self.call(*callee, &args, inst.span)?
                    }
                    InstKind::CallRuntime(rt, args) => {
                        let args: Vec<BasicValueEnum<'ctx>> =
                            args.iter().map(|a| get(*a)).collect();
                        self.call_runtime(*rt, &args)?
                    }
                    InstKind::MakeStruct(id, args) => {
                        let mut value = self.struct_types[id.index()].get_undef().into();
                        for (index, arg) in args.iter().enumerate() {
                            value = self.insert(value, index, get(*arg))?;
                        }
                        Some(value)
                    }
                    InstKind::Extract(v, index) => {
                        let value = get(*v).into_struct_value();
                        Some(self.builder.build_extract_value(value, *index as u32, "")?)
                    }
                    InstKind::Insert(v, index, x) => Some(self.insert(get(*v), *index, get(*x))?),
                };
                if let (Some(dest), Some(value)) = (inst.result, result) {
                    let ty = f.values[dest.index()];
                    if self.managed(ty) {
                        // Calls return their result at +1; everything else
                        // takes its own reference.
                        let call =
                            matches!(inst.kind, InstKind::Call(..) | InstKind::CallRuntime(..));
                        let constant = matches!(inst.kind, InstKind::Const(_));
                        if !call && !constant {
                            self.retain_value(value, ty)?;
                        }
                        if !constant {
                            owned.push((value, ty));
                        }
                    }
                    values[dest.index()] = Some(value);
                }
            }
            let returned = match &block.term {
                Terminator::Return(Some(v)) => {
                    Some(values[v.index()].expect("validated IR: returned value is defined"))
                }
                _ => None,
            };
            // The caller receives a returned value at +1.
            if let Some(v) = returned {
                self.retain_value(v, f.ret)?;
            }
            if !matches!(block.term, Terminator::Unreachable) {
                for (value, ty) in owned {
                    self.release_value(value, ty)?;
                }
            }
            if matches!(block.term, Terminator::Return(_)) {
                for &(ptr, ty) in &managed_slots {
                    let value = self.builder.build_load(self.basic_type(ty), ptr, "")?;
                    self.release_value(value, ty)?;
                }
            }
            match &block.term {
                Terminator::Return(None) => {
                    self.builder.build_return(None)?;
                }
                Terminator::Return(Some(_)) => {
                    let v = returned.expect("set above");
                    self.builder.build_return(Some(&v))?;
                }
                Terminator::Jump(target) => {
                    self.builder
                        .build_unconditional_branch(blocks[target.index()])?;
                }
                Terminator::Branch {
                    cond,
                    then_block,
                    else_block,
                } => {
                    let cond = values[cond.index()]
                        .expect("validated IR: condition is defined")
                        .into_int_value();
                    self.builder.build_conditional_branch(
                        cond,
                        blocks[then_block.index()],
                        blocks[else_block.index()],
                    )?;
                }
                Terminator::Unreachable => {
                    self.builder.build_unreachable()?;
                }
            }
        }
        Ok(())
    }

    // ---- instructions --------------------------------------------------

    fn constant(&mut self, c: &Const) -> BasicValueEnum<'ctx> {
        match c {
            Const::I64(v) => self.context.i64_type().const_int(*v as u64, true).into(),
            Const::Bool(v) => self
                .context
                .bool_type()
                .const_int(u64::from(*v), false)
                .into(),
            Const::Str(s) => self.brim_str(s).into(),
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        use BinaryOp::*;
        let compare = |pred| self.builder.build_int_compare(pred, a, b, "");
        Ok(match op {
            IAdd => self.checked("llvm.sadd.with.overflow", a, b, "+", span)?,
            ISub => self.checked("llvm.ssub.with.overflow", a, b, "-", span)?,
            IMul => self.checked("llvm.smul.with.overflow", a, b, "*", span)?,
            IDiv => {
                self.check_divisor(a, b, "/", span)?;
                self.builder.build_int_signed_div(a, b, "")?.into()
            }
            IRem => {
                self.check_divisor(a, b, "%", span)?;
                self.builder.build_int_signed_rem(a, b, "")?.into()
            }
            IEq | BEq => compare(IntPredicate::EQ)?.into(),
            INe | BNe => compare(IntPredicate::NE)?.into(),
            ILt => compare(IntPredicate::SLT)?.into(),
            ILe => compare(IntPredicate::SLE)?.into(),
            IGt => compare(IntPredicate::SGT)?.into(),
            IGe => compare(IntPredicate::SGE)?.into(),
        })
    }

    /// `a op b` through an overflow intrinsic, trapping on overflow.
    fn checked(
        &mut self,
        intrinsic: &str,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
        op: &'static str,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let declaration = Intrinsic::find(intrinsic)
            .and_then(|i| i.get_declaration(&self.module, &[self.context.i64_type().into()]))
            .expect("LLVM provides the overflow intrinsics");
        let pair = self
            .builder
            .build_call(declaration, &[a.into(), b.into()], "")?
            .try_as_basic_value()
            .basic()
            .expect("overflow intrinsics return a value")
            .into_struct_value();
        let result = self.builder.build_extract_value(pair, 0, "")?;
        let overflow = self
            .builder
            .build_extract_value(pair, 1, "")?
            .into_int_value();
        self.trap_if(overflow, &RuntimeError::IntegerOverflow { op }, span)?;
        Ok(result)
    }

    /// Division by zero, and `i64::MIN / -1` (which overflows), trap.
    fn check_divisor(
        &mut self,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
        op: &'static str,
        span: Option<Span>,
    ) -> Result<(), BuilderError> {
        let i64_type = self.context.i64_type();
        let is_zero =
            self.builder
                .build_int_compare(IntPredicate::EQ, b, i64_type.const_zero(), "")?;
        self.trap_if(is_zero, &RuntimeError::DivisionByZero { op }, span)?;
        let is_min = self.builder.build_int_compare(
            IntPredicate::EQ,
            a,
            i64_type.const_int(i64::MIN as u64, true),
            "",
        )?;
        let is_minus_one =
            self.builder
                .build_int_compare(IntPredicate::EQ, b, i64_type.const_all_ones(), "")?;
        let overflow = self.builder.build_and(is_min, is_minus_one, "")?;
        self.trap_if(overflow, &RuntimeError::IntegerOverflow { op }, span)
    }

    fn call(
        &mut self,
        callee: FuncId,
        args: &[BasicMetadataValueEnum<'ctx>],
        span: Option<Span>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, BuilderError> {
        let name = self.c_string(&self.ir.function(callee).name.clone());
        let [head, sep, note] = self.trap_text(
            &RuntimeError::StackOverflow {
                limit: MAX_CALL_DEPTH,
            },
            span,
        );
        self.builder.build_call(
            self.enter,
            &[name.into(), head.into(), sep.into(), note.into()],
            "",
        )?;
        let result = self
            .builder
            .build_call(self.functions[callee.index()], args, "")?
            .try_as_basic_value()
            .basic();
        self.builder.build_call(self.leave, &[], "")?;
        Ok(result)
    }

    fn call_runtime(
        &mut self,
        rt: RuntimeFn,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<Option<BasicValueEnum<'ctx>>, BuilderError> {
        let function = self.runtime_fn(rt);
        let i8_type = self.context.i8_type();
        let mut abi_args: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(rt.params()) {
            abi_args.push(match ty {
                AbiType::Bool => self
                    .builder
                    .build_int_z_extend(arg.into_int_value(), i8_type, "")?
                    .into(),
                _ => (*arg).into(),
            });
        }
        let result = self
            .builder
            .build_call(function, &abi_args, "")?
            .try_as_basic_value()
            .basic();
        Ok(match (result, rt.ret()) {
            (Some(v), AbiType::Bool) => Some(
                self.builder
                    .build_int_compare(
                        IntPredicate::NE,
                        v.into_int_value(),
                        i8_type.const_zero(),
                        "",
                    )?
                    .into(),
            ),
            (result, _) => result,
        })
    }

    /// Branches to a call of `brim_rt_trap` when `cond` is true.
    fn trap_if(
        &mut self,
        cond: IntValue<'ctx>,
        error: &RuntimeError,
        span: Option<Span>,
    ) -> Result<(), BuilderError> {
        let function = self
            .builder
            .get_insert_block()
            .and_then(|b| b.get_parent())
            .expect("the builder is inside a function");
        let trap_block = self.context.append_basic_block(function, "trap");
        let continue_block = self.context.append_basic_block(function, "ok");
        self.builder
            .build_conditional_branch(cond, trap_block, continue_block)?;
        self.builder.position_at_end(trap_block);
        let [head, sep, note] = self.trap_text(error, span);
        self.builder
            .build_call(self.trap, &[head.into(), sep.into(), note.into()], "")?;
        self.builder.build_unreachable()?;
        self.builder.position_at_end(continue_block);
        Ok(())
    }

    fn insert(
        &mut self,
        aggregate: BasicValueEnum<'ctx>,
        index: usize,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let aggregate = aggregate.into_struct_value();
        Ok(self
            .builder
            .build_insert_value(aggregate, value, index as u32, "")?
            .into_struct_value()
            .into())
    }

    // ---- reference counting ---------------------------------------------

    /// Whether values of `ty` contain strings, and so need reference counting.
    fn managed(&self, ty: Type) -> bool {
        match ty {
            Type::Str => true,
            Type::Struct(id) => self.managed_structs[id.index()],
            Type::I64 | Type::Bool | Type::Void => false,
        }
    }

    /// Takes a reference to every string in `value`.
    fn retain_value(&mut self, value: BasicValueEnum<'ctx>, ty: Type) -> Result<(), BuilderError> {
        self.count_value(value, ty, self.retain)
    }

    /// Drops a reference to every string in `value`.
    fn release_value(&mut self, value: BasicValueEnum<'ctx>, ty: Type) -> Result<(), BuilderError> {
        self.count_value(value, ty, self.release)
    }

    fn count_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        ty: Type,
        function: FunctionValue<'ctx>,
    ) -> Result<(), BuilderError> {
        match ty {
            Type::Str => {
                self.builder.build_call(function, &[value.into()], "")?;
            }
            Type::Struct(id) if self.managed(ty) => {
                let fields: Vec<Type> =
                    self.ir.struct_def(id).fields.iter().map(|f| f.ty).collect();
                for (index, field_ty) in fields.into_iter().enumerate() {
                    if self.managed(field_ty) {
                        let field = self.builder.build_extract_value(
                            value.into_struct_value(),
                            index as u32,
                            "",
                        )?;
                        self.count_value(field, field_ty, function)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ---- declarations and constants -------------------------------------

    fn basic_type(&self, ty: Type) -> BasicTypeEnum<'ctx> {
        match ty {
            Type::I64 => self.context.i64_type().into(),
            Type::Bool => self.context.bool_type().into(),
            Type::Str => self.context.ptr_type(AddressSpace::default()).into(),
            Type::Struct(id) => self.struct_types[id.index()].into(),
            Type::Void => unreachable!("validated IR: `void` is only a return type"),
        }
    }

    fn fn_type(&self, ret: Type, params: &[Type]) -> FunctionType<'ctx> {
        let params: Vec<BasicMetadataTypeEnum<'ctx>> =
            params.iter().map(|t| self.basic_type(*t).into()).collect();
        match ret {
            Type::Void => self.context.void_type().fn_type(&params, false),
            ty => self.basic_type(ty).fn_type(&params, false),
        }
    }

    /// The external declaration of a runtime function, in ABI types.
    fn runtime_fn(&mut self, rt: RuntimeFn) -> FunctionValue<'ctx> {
        if let Some(f) = self.runtime.get(&rt) {
            return *f;
        }
        let abi = |ty: AbiType| -> BasicTypeEnum<'ctx> {
            match ty {
                AbiType::I64 => self.context.i64_type().into(),
                AbiType::Bool => self.context.i8_type().into(),
                AbiType::Str => self.context.ptr_type(AddressSpace::default()).into(),
                AbiType::Void => unreachable!("`void` is never a runtime parameter"),
            }
        };
        let params: Vec<BasicMetadataTypeEnum<'ctx>> =
            rt.params().iter().map(|t| abi(*t).into()).collect();
        let fn_type = match rt.ret() {
            AbiType::Void => self.context.void_type().fn_type(&params, false),
            ty => abi(ty).fn_type(&params, false),
        };
        let f = self
            .module
            .add_function(rt.symbol(), fn_type, Some(Linkage::External));
        self.runtime.insert(rt, f);
        f
    }

    /// A constant `BrimStr` object: `{ i64 -1, i64 len, [len x i8] }`.
    fn brim_str(&mut self, text: &str) -> PointerValue<'ctx> {
        if let Some(p) = self.strings.get(text) {
            return *p;
        }
        let i64_type = self.context.i64_type();
        let value = self.context.const_struct(
            &[
                i64_type.const_all_ones().into(),
                i64_type.const_int(text.len() as u64, false).into(),
                self.context.const_string(text.as_bytes(), false).into(),
            ],
            false,
        );
        let global = self.module.add_global(value.get_type(), None, "str");
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        global.set_alignment(8);
        let ptr = global.as_pointer_value();
        self.strings.insert(text.to_string(), ptr);
        ptr
    }

    /// A NUL-terminated constant string, for the runtime's messages.
    fn c_string(&mut self, text: &str) -> PointerValue<'ctx> {
        if let Some(p) = self.c_strings.get(text) {
            return *p;
        }
        let value = self.context.const_string(text.as_bytes(), true);
        let global = self.module.add_global(value.get_type(), None, "cstr");
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        let ptr = global.as_pointer_value();
        self.c_strings.insert(text.to_string(), ptr);
        ptr
    }

    /// The pre-rendered message parts for `brim_rt_trap`.
    fn trap_text(&mut self, error: &RuntimeError, span: Option<Span>) -> [PointerValue<'ctx>; 3] {
        let TrapText { head, sep, note } = trap::render(error, span, self.source);
        [
            self.c_string(&head),
            self.c_string(&sep),
            self.c_string(&note),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_ir::{Block, Function, Inst, Slot, SlotId, ValueId};

    fn inst(result: Option<u32>, kind: InstKind) -> Inst {
        Inst {
            result: result.map(ValueId),
            kind,
            span: Some(Span::new(0, 1)),
        }
    }

    /// `fn div(a: i64, b: i64) -> i64 { return a / b }` and a `main` that
    /// prints `div(7, 2)` and `"done"`.
    fn sample() -> IrModule {
        let div = Function {
            name: "div".into(),
            param_count: 2,
            ret: Type::I64,
            slots: vec![
                Slot {
                    name: "a".into(),
                    ty: Type::I64,
                },
                Slot {
                    name: "b".into(),
                    ty: Type::I64,
                },
            ],
            values: vec![Type::I64; 3],
            blocks: vec![Block {
                insts: vec![
                    inst(Some(0), InstKind::Load(SlotId(0))),
                    inst(Some(1), InstKind::Load(SlotId(1))),
                    inst(
                        Some(2),
                        InstKind::Binary(BinaryOp::IDiv, ValueId(0), ValueId(1)),
                    ),
                ],
                term: Terminator::Return(Some(ValueId(2))),
            }],
        };
        let main = Function {
            name: "main".into(),
            param_count: 0,
            ret: Type::Void,
            slots: vec![],
            values: vec![Type::I64, Type::I64, Type::I64, Type::Str],
            blocks: vec![Block {
                insts: vec![
                    inst(Some(0), InstKind::Const(Const::I64(7))),
                    inst(Some(1), InstKind::Const(Const::I64(2))),
                    inst(
                        Some(2),
                        InstKind::Call(FuncId(0), vec![ValueId(0), ValueId(1)]),
                    ),
                    inst(
                        None,
                        InstKind::CallRuntime(RuntimeFn::PrintI64, vec![ValueId(2)]),
                    ),
                    inst(Some(3), InstKind::Const(Const::Str("done".into()))),
                    inst(
                        None,
                        InstKind::CallRuntime(RuntimeFn::PrintStr, vec![ValueId(3)]),
                    ),
                ],
                term: Terminator::Return(None),
            }],
        };
        IrModule {
            structs: vec![],
            functions: vec![div, main],
            entry: FuncId(1),
        }
    }

    #[test]
    fn lowers_to_valid_llvm_ir() {
        let source = SourceFile::new("t.brim", "x\n");
        let ir = emit_ir(&sample(), &source).unwrap();
        for expected in [
            "define internal i64 @brim.div(i64 %0, i64 %1)",
            "define internal void @brim.main()",
            "define void @brim_entry()",
            "@brim_max_call_depth = constant i64 10000",
            "sdiv i64",
            "call void @brim_rt_print_i64(i64",
            "call void @brim_rt_print_str(ptr",
            "{ i64 -1, i64 4, [4 x i8] c\"done\" }",
            "error[R0002]: division by zero",
            "error[R0001]: integer overflow in `/`",
            "error[R0003]: stack overflow: more than 10000 nested calls",
        ] {
            assert!(ir.contains(expected), "missing `{expected}` in:\n{ir}");
        }
    }

    #[test]
    fn strings_are_reference_counted() {
        // fn echo(s: string) -> string { let t: string = s; return t }
        let echo = Function {
            name: "echo".into(),
            param_count: 1,
            ret: Type::Str,
            slots: vec![
                Slot {
                    name: "s".into(),
                    ty: Type::Str,
                },
                Slot {
                    name: "t".into(),
                    ty: Type::Str,
                },
            ],
            values: vec![Type::Str, Type::Str],
            blocks: vec![Block {
                insts: vec![
                    inst(Some(0), InstKind::Load(SlotId(0))),
                    inst(None, InstKind::Store(SlotId(1), ValueId(0))),
                    inst(Some(1), InstKind::Load(SlotId(1))),
                ],
                term: Terminator::Return(Some(ValueId(1))),
            }],
        };
        let main = Function {
            name: "main".into(),
            param_count: 0,
            ret: Type::Void,
            slots: vec![],
            values: vec![],
            blocks: vec![Block {
                insts: vec![],
                term: Terminator::Return(None),
            }],
        };
        let module = IrModule {
            structs: vec![],
            functions: vec![echo, main],
            entry: FuncId(1),
        };
        let ir = emit_ir(&module, &SourceFile::new("t.brim", "x\n")).unwrap();
        let body = &ir[ir.find("@brim.echo(").unwrap()..];
        let body = &body[..body.find("\n}").unwrap()];
        // Parameter slot (1), two loads (2), the store (1) and the return (1).
        assert_eq!(
            body.matches("call void @brim_rt_retain").count(),
            5,
            "{body}"
        );
        // The store's old value (1), two loads (2) and both slots at return (2).
        assert_eq!(
            body.matches("call void @brim_rt_release").count(),
            5,
            "{body}"
        );
        assert!(body.contains("store ptr null"), "{body}");
    }

    #[test]
    fn identical_strings_are_shared() {
        let mut module = sample();
        let main = &mut module.functions[1];
        main.values.push(Type::Str);
        main.blocks[0].insts.extend([
            inst(Some(4), InstKind::Const(Const::Str("done".into()))),
            inst(
                None,
                InstKind::CallRuntime(RuntimeFn::PrintStr, vec![ValueId(4)]),
            ),
        ]);
        let ir = emit_ir(&module, &SourceFile::new("t.brim", "x\n")).unwrap();
        assert_eq!(ir.matches("c\"done\"").count(), 1, "{ir}");
    }
}
