//! Lists in generated code: calls into the runtime's `brim_rt_list_*`
//! functions (see `runtime/brim_rt.c`).
//!
//! A list value is a pointer to a runtime list object (null for an empty
//! list). Elements are passed to and from the runtime by pointer: values are
//! stored in a stack slot first, and element reads load through the pointer
//! the runtime returns. For element types that contain strings or lists, each
//! list type gets two small functions that retain and release one element,
//! which the runtime calls when it copies or frees elements.

use brim_diagnostics::Span;
use brim_ir::{ListId, Type};
use inkwell::AddressSpace;
use inkwell::builder::BuilderError;
use inkwell::module::Linkage;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};

use super::Lowerer;
use crate::trap::{self, IndexTrapText};

/// The runtime functions for lists.
pub(super) struct ListRuntime<'ctx> {
    pub retain: FunctionValue<'ctx>,
    pub release: FunctionValue<'ctx>,
    len: FunctionValue<'ctx>,
    at: FunctionValue<'ctx>,
    push: FunctionValue<'ctx>,
    set: FunctionValue<'ctx>,
    truncate: FunctionValue<'ctx>,
    insert: FunctionValue<'ctx>,
    remove: FunctionValue<'ctx>,
    contains_i64: FunctionValue<'ctx>,
    contains_bool: FunctionValue<'ctx>,
    contains_str: FunctionValue<'ctx>,
    join: FunctionValue<'ctx>,
}

/// What the runtime needs to know about a list type's elements.
#[derive(Clone, Copy)]
pub(super) struct ElemInfo<'ctx> {
    size: IntValue<'ctx>,
    /// Null when elements need no reference counting.
    retain: PointerValue<'ctx>,
    release: PointerValue<'ctx>,
}

impl<'ctx> ListRuntime<'ctx> {
    pub(super) fn declare(
        context: &'ctx inkwell::context::Context,
        module: &inkwell::module::Module<'ctx>,
    ) -> Self {
        let ptr = context.ptr_type(AddressSpace::default());
        let i64_type = context.i64_type();
        let i8_type = context.i8_type();
        let void = context.void_type();
        let declare = |name: &str, ty| module.add_function(name, ty, Some(Linkage::External));
        // `elem_size, retain, release` follow the element on changing calls.
        let changing = |extra: &[inkwell::types::BasicMetadataTypeEnum<'ctx>]| {
            let mut params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = vec![ptr.into()];
            params.extend_from_slice(extra);
            params.extend::<[inkwell::types::BasicMetadataTypeEnum<'ctx>; 3]>([
                i64_type.into(),
                ptr.into(),
                ptr.into(),
            ]);
            params
        };
        ListRuntime {
            retain: declare("brim_rt_list_retain", void.fn_type(&[ptr.into()], false)),
            release: declare("brim_rt_list_release", void.fn_type(&[ptr.into()], false)),
            len: declare("brim_rt_list_len", i64_type.fn_type(&[ptr.into()], false)),
            at: declare(
                "brim_rt_list_at",
                ptr.fn_type(&[ptr.into(), i64_type.into(), ptr.into()], false),
            ),
            push: declare(
                "brim_rt_list_push",
                ptr.fn_type(&changing(&[ptr.into()]), false),
            ),
            set: declare("brim_rt_list_set", {
                let mut p = changing(&[i64_type.into(), ptr.into()]);
                p.push(ptr.into());
                ptr.fn_type(&p, false)
            }),
            truncate: declare(
                "brim_rt_list_truncate",
                ptr.fn_type(&changing(&[i64_type.into()]), false),
            ),
            insert: declare("brim_rt_list_insert", {
                let mut p = changing(&[i64_type.into(), ptr.into()]);
                p.push(ptr.into());
                ptr.fn_type(&p, false)
            }),
            remove: declare("brim_rt_list_remove", {
                let mut p = changing(&[i64_type.into()]);
                p.push(ptr.into());
                ptr.fn_type(&p, false)
            }),
            contains_i64: declare(
                "brim_rt_list_contains_i64",
                i8_type.fn_type(&[ptr.into(), i64_type.into()], false),
            ),
            contains_bool: declare(
                "brim_rt_list_contains_bool",
                i8_type.fn_type(&[ptr.into(), i8_type.into()], false),
            ),
            contains_str: declare(
                "brim_rt_list_contains_str",
                i8_type.fn_type(&[ptr.into(), ptr.into()], false),
            ),
            join: declare(
                "brim_rt_list_join",
                ptr.fn_type(&[ptr.into(), ptr.into()], false),
            ),
        }
    }
}

impl<'ctx> Lowerer<'ctx, '_> {
    /// Element sizes and retain/release helpers for every list type. Called
    /// before any function body is lowered.
    pub(super) fn declare_list_elements(&mut self) -> Result<(), BuilderError> {
        for (index, &elem) in self.ir.lists.iter().enumerate() {
            let (size, retain, release) = self.element_helpers(elem, &format!("L{index}"))?;
            self.list_elems.push(ElemInfo {
                size,
                retain,
                release,
            });
        }
        Ok(())
    }

    /// The size of values of `ty` in memory, and functions that retain and
    /// release one such value through a pointer (null when `ty` needs no
    /// reference counting). `label` names the helpers.
    pub(super) fn element_helpers(
        &mut self,
        ty: Type,
        label: &str,
    ) -> Result<(IntValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>), BuilderError> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        let null = ptr.const_null();
        let llvm_ty = self.basic_type(ty);
        let size = self
            .context
            .i64_type()
            .const_int(self.target_data.get_abi_size(&llvm_ty), false);
        if !self.managed(ty) {
            return Ok((size, null, null));
        }
        let mut helper = |name: &str, retain: bool| -> Result<PointerValue<'ctx>, BuilderError> {
            let function = self.module.add_function(
                &format!("brim.elem.{name}.{label}"),
                self.context.void_type().fn_type(&[ptr.into()], false),
                Some(Linkage::Internal),
            );
            let block = self.context.append_basic_block(function, "entry");
            self.builder.position_at_end(block);
            let at = function
                .get_nth_param(0)
                .expect("one parameter")
                .into_pointer_value();
            let value = self.builder.build_load(llvm_ty, at, "")?;
            if retain {
                self.retain_value(value, ty)?;
            } else {
                self.release_value(value, ty)?;
            }
            self.builder.build_return(None)?;
            Ok(function.as_global_value().as_pointer_value())
        };
        Ok((size, helper("retain", true)?, helper("release", false)?))
    }

    fn elem_info(&self, list: Type) -> (ElemInfo<'ctx>, Type) {
        let Type::List(id) = list else {
            unreachable!("validated IR: a list operand");
        };
        (self.list_elems[id.index()], self.ir.list_elem(id))
    }

    /// Stores `value` in the stack slot for its element type and returns the
    /// slot's address.
    fn spill_elem(
        &mut self,
        value: BasicValueEnum<'ctx>,
        elem: Type,
    ) -> Result<PointerValue<'ctx>, BuilderError> {
        let slot = self.stack_slot(elem, false)?;
        self.builder.build_store(slot, value)?;
        Ok(slot)
    }

    /// The constant describing an index error at `span`.
    fn index_trap(&mut self, span: Option<Span>) -> PointerValue<'ctx> {
        let IndexTrapText {
            prefix,
            suffix,
            sep,
            note,
        } = trap::render_index(span, self.source);
        let key = format!("{prefix}\u{0}{suffix}\u{0}{sep}\u{0}{note}");
        if let Some(p) = self.index_traps.get(&key) {
            return *p;
        }
        let parts = [
            self.c_string(&prefix),
            self.c_string(&suffix),
            self.c_string(&sep),
            self.c_string(&note),
        ];
        let value = self.context.const_struct(&parts.map(|p| p.into()), false);
        let global = self.module.add_global(value.get_type(), None, "index_trap");
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        let ptr = global.as_pointer_value();
        self.index_traps.insert(key, ptr);
        ptr
    }

    pub(super) fn call_value(
        &mut self,
        function: FunctionValue<'ctx>,
        args: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let args: Vec<_> = args.iter().map(|a| (*a).into()).collect();
        Ok(self
            .builder
            .build_call(function, &args, "")?
            .try_as_basic_value()
            .basic()
            .expect("the list runtime function returns a value"))
    }

    pub(super) fn list_new(
        &mut self,
        id: ListId,
        items: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let mut list: BasicValueEnum<'ctx> = self
            .context
            .ptr_type(AddressSpace::default())
            .const_null()
            .into();
        for item in items {
            list = self.list_push(Type::List(id), list, *item)?;
        }
        Ok(list)
    }

    pub(super) fn list_len(
        &mut self,
        list: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let len = self.lists.len;
        self.call_value(len, &[list])
    }

    pub(super) fn list_get(
        &mut self,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        index: BasicValueEnum<'ctx>,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (_, elem) = self.elem_info(list_ty);
        let trap = self.index_trap(span);
        let at = self.lists.at;
        let ptr = self.call_value(at, &[list, index, trap.into()])?;
        self.builder
            .build_load(self.basic_type(elem), ptr.into_pointer_value(), "")
    }

    pub(super) fn list_push(
        &mut self,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, elem) = self.elem_info(list_ty);
        let slot = self.spill_elem(value, elem)?;
        let push = self.lists.push;
        self.call_value(
            push,
            &[
                list,
                slot.into(),
                info.size.into(),
                info.retain.into(),
                info.release.into(),
            ],
        )
    }

    /// `set` and `insert`, which take an index, an element and a trap.
    pub(super) fn list_put(
        &mut self,
        insert: bool,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        index: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, elem) = self.elem_info(list_ty);
        let slot = self.spill_elem(value, elem)?;
        let trap = self.index_trap(span);
        let function = if insert {
            self.lists.insert
        } else {
            self.lists.set
        };
        self.call_value(
            function,
            &[
                list,
                index,
                slot.into(),
                info.size.into(),
                info.retain.into(),
                info.release.into(),
                trap.into(),
            ],
        )
    }

    pub(super) fn list_truncate(
        &mut self,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        n: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, _) = self.elem_info(list_ty);
        let truncate = self.lists.truncate;
        self.call_value(
            truncate,
            &[
                list,
                n,
                info.size.into(),
                info.retain.into(),
                info.release.into(),
            ],
        )
    }

    pub(super) fn list_remove(
        &mut self,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        index: BasicValueEnum<'ctx>,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, _) = self.elem_info(list_ty);
        let trap = self.index_trap(span);
        let remove = self.lists.remove;
        self.call_value(
            remove,
            &[
                list,
                index,
                info.size.into(),
                info.retain.into(),
                info.release.into(),
                trap.into(),
            ],
        )
    }

    pub(super) fn list_contains(
        &mut self,
        list_ty: Type,
        list: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (_, elem) = self.elem_info(list_ty);
        let i8_type = self.context.i8_type();
        let (function, value) = match elem {
            Type::I64 => (self.lists.contains_i64, value),
            Type::Bool => (
                self.lists.contains_bool,
                self.builder
                    .build_int_z_extend(value.into_int_value(), i8_type, "")?
                    .into(),
            ),
            Type::Str => (self.lists.contains_str, value),
            other => unreachable!("validated IR: list.contains on {other}"),
        };
        let found = self.call_value(function, &[list, value])?;
        Ok(self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                found.into_int_value(),
                i8_type.const_zero(),
                "",
            )?
            .into())
    }

    pub(super) fn list_join(
        &mut self,
        list: BasicValueEnum<'ctx>,
        sep: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let join = self.lists.join;
        self.call_value(join, &[list, sep])
    }

    /// A stack slot for passing values of `ty` to the runtime, in the
    /// function's entry block (so a loop does not grow the stack). Keys and
    /// other values get separate slots, as a call may need both.
    pub(super) fn stack_slot(
        &mut self,
        ty: Type,
        key: bool,
    ) -> Result<PointerValue<'ctx>, BuilderError> {
        if let Some(slot) = self.elem_slots.get(&(ty, key)) {
            return Ok(*slot);
        }
        let current = self
            .builder
            .get_insert_block()
            .expect("the builder is inside a function");
        let function = current.get_parent().expect("blocks belong to functions");
        let entry = function
            .get_first_basic_block()
            .expect("functions have an entry block");
        match entry.get_first_instruction() {
            Some(first) => self.builder.position_before(&first),
            None => self.builder.position_at_end(entry),
        }
        let slot = self.builder.build_alloca(self.basic_type(ty), "elem")?;
        self.builder.position_at_end(current);
        self.elem_slots.insert((ty, key), slot);
        Ok(slot)
    }
}
