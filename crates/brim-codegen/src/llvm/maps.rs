//! Maps in generated code: calls into the runtime's `brim_rt_map_*`
//! functions. Keys and values cross the boundary by pointer, like list
//! elements; values use the same kind of generated retain/release helpers.

use brim_diagnostics::Span;
use brim_ir::{MapId, Type};
use brim_runtime::RuntimeError;
use inkwell::AddressSpace;
use inkwell::builder::BuilderError;
use inkwell::module::Linkage;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};

use super::Lowerer;

/// The runtime functions for maps.
pub(super) struct MapRuntime<'ctx> {
    pub retain: FunctionValue<'ctx>,
    pub release: FunctionValue<'ctx>,
    len: FunctionValue<'ctx>,
    find: FunctionValue<'ctx>,
    index: FunctionValue<'ctx>,
    key_at: FunctionValue<'ctx>,
    value_at: FunctionValue<'ctx>,
    insert: FunctionValue<'ctx>,
    set_value_at: FunctionValue<'ctx>,
    remove_at: FunctionValue<'ctx>,
}

/// What the runtime needs to know about a map type.
#[derive(Clone, Copy)]
pub(super) struct MapInfo<'ctx> {
    key_kind: IntValue<'ctx>,
    value_size: IntValue<'ctx>,
    retain: PointerValue<'ctx>,
    release: PointerValue<'ctx>,
}

impl<'ctx> MapRuntime<'ctx> {
    pub(super) fn declare(
        context: &'ctx inkwell::context::Context,
        module: &inkwell::module::Module<'ctx>,
    ) -> Self {
        let ptr = context.ptr_type(AddressSpace::default());
        let i64_type = context.i64_type();
        let void = context.void_type();
        let declare = |name: &str, ty| module.add_function(name, ty, Some(Linkage::External));
        // Changing calls end with `key_kind, value_size, retain, release`.
        let changing = |first: &[BasicMetadataTypeEnum<'ctx>]| {
            let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr.into()];
            params.extend_from_slice(first);
            params.extend::<[BasicMetadataTypeEnum<'ctx>; 4]>([
                i64_type.into(),
                i64_type.into(),
                ptr.into(),
                ptr.into(),
            ]);
            ptr.fn_type(&params, false)
        };
        MapRuntime {
            retain: declare("brim_rt_map_retain", void.fn_type(&[ptr.into()], false)),
            release: declare("brim_rt_map_release", void.fn_type(&[ptr.into()], false)),
            len: declare("brim_rt_map_len", i64_type.fn_type(&[ptr.into()], false)),
            find: declare(
                "brim_rt_map_find",
                i64_type.fn_type(&[ptr.into(), ptr.into()], false),
            ),
            index: declare(
                "brim_rt_map_index",
                i64_type.fn_type(
                    &[ptr.into(), ptr.into(), ptr.into(), ptr.into(), ptr.into()],
                    false,
                ),
            ),
            key_at: declare(
                "brim_rt_map_key_at",
                ptr.fn_type(&[ptr.into(), i64_type.into()], false),
            ),
            value_at: declare(
                "brim_rt_map_value_at",
                ptr.fn_type(&[ptr.into(), i64_type.into()], false),
            ),
            insert: declare("brim_rt_map_insert", changing(&[ptr.into(), ptr.into()])),
            set_value_at: declare(
                "brim_rt_map_set_value_at",
                changing(&[i64_type.into(), ptr.into()]),
            ),
            remove_at: declare("brim_rt_map_remove_at", changing(&[i64_type.into()])),
        }
    }
}

impl<'ctx> Lowerer<'ctx, '_> {
    /// Key kinds, value sizes and value retain/release helpers for every map
    /// type. Called before any function body is lowered.
    pub(super) fn declare_map_types(&mut self) -> Result<(), BuilderError> {
        let i64_type = self.context.i64_type();
        for (index, &(key, value)) in self.ir.maps.iter().enumerate() {
            let key_kind = match key {
                Type::I64 => 0,
                Type::Bool => 1,
                Type::Str => 2,
                other => unreachable!("validated IR: map key type {other}"),
            };
            let (value_size, retain, release) =
                self.element_helpers(value, &format!("M{index}"))?;
            self.map_infos.push(MapInfo {
                key_kind: i64_type.const_int(key_kind, false),
                value_size,
                retain,
                release,
            });
        }
        Ok(())
    }

    fn map_info(&self, map: Type) -> (MapInfo<'ctx>, Type, Type) {
        let Type::Map(id) = map else {
            unreachable!("validated IR: a map operand");
        };
        let (key, value) = self.ir.map_types(id);
        (self.map_infos[id.index()], key, value)
    }

    pub(super) fn map_new(&self, _id: MapId) -> BasicValueEnum<'ctx> {
        self.context
            .ptr_type(AddressSpace::default())
            .const_null()
            .into()
    }

    pub(super) fn map_len(
        &mut self,
        map: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let len = self.maps.len;
        self.call_value(len, &[map])
    }

    /// `brim_rt_map_find` (`trap` false) or `brim_rt_map_index`.
    pub(super) fn map_position(
        &mut self,
        trap: bool,
        map_ty: Type,
        map: BasicValueEnum<'ctx>,
        key: BasicValueEnum<'ctx>,
        span: Option<Span>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (_, key_ty, _) = self.map_info(map_ty);
        let key_slot = self.stack_slot(key_ty, true)?;
        self.builder.build_store(key_slot, key)?;
        if trap {
            let [head, sep, note] = self.trap_text(&RuntimeError::KeyNotFound, span);
            let index = self.maps.index;
            self.call_value(
                index,
                &[map, key_slot.into(), head.into(), sep.into(), note.into()],
            )
        } else {
            let find = self.maps.find;
            self.call_value(find, &[map, key_slot.into()])
        }
    }

    /// The key (`key` true) or value at a position.
    pub(super) fn map_entry(
        &mut self,
        key: bool,
        map_ty: Type,
        map: BasicValueEnum<'ctx>,
        at: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (_, key_ty, value_ty) = self.map_info(map_ty);
        let (function, ty) = if key {
            (self.maps.key_at, key_ty)
        } else {
            (self.maps.value_at, value_ty)
        };
        let ptr = self.call_value(function, &[map, at])?;
        self.builder
            .build_load(self.basic_type(ty), ptr.into_pointer_value(), "")
    }

    pub(super) fn map_insert(
        &mut self,
        map_ty: Type,
        map: BasicValueEnum<'ctx>,
        key: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, key_ty, value_ty) = self.map_info(map_ty);
        let key_slot = self.stack_slot(key_ty, true)?;
        self.builder.build_store(key_slot, key)?;
        let value_slot = self.stack_slot(value_ty, false)?;
        self.builder.build_store(value_slot, value)?;
        let insert = self.maps.insert;
        self.call_value(
            insert,
            &[
                map,
                key_slot.into(),
                value_slot.into(),
                info.key_kind.into(),
                info.value_size.into(),
                info.retain.into(),
                info.release.into(),
            ],
        )
    }

    pub(super) fn map_set_value_at(
        &mut self,
        map_ty: Type,
        map: BasicValueEnum<'ctx>,
        at: BasicValueEnum<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, _, value_ty) = self.map_info(map_ty);
        let value_slot = self.stack_slot(value_ty, false)?;
        self.builder.build_store(value_slot, value)?;
        let set = self.maps.set_value_at;
        self.call_value(
            set,
            &[
                map,
                at,
                value_slot.into(),
                info.key_kind.into(),
                info.value_size.into(),
                info.retain.into(),
                info.release.into(),
            ],
        )
    }

    pub(super) fn map_remove_at(
        &mut self,
        map_ty: Type,
        map: BasicValueEnum<'ctx>,
        at: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, BuilderError> {
        let (info, _, _) = self.map_info(map_ty);
        let remove = self.maps.remove_at;
        self.call_value(
            remove,
            &[
                map,
                at,
                info.key_kind.into(),
                info.value_size.into(),
                info.retain.into(),
                info.release.into(),
            ],
        )
    }
}
