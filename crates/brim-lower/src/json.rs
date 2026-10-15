//! Values to and from std.json's `Json`, generated per concrete type for
//! `json.encode`, `json.decode` and API functions.
//!
//! The shapes are those `brim gen ts` declares: structs are objects, an
//! `Option` is its value or `null`, an enum without payloads is a variant
//! name and other enums `{"kind": ..., "value": ...}`, tuples and sets are
//! arrays and maps are objects with their keys as text. The leaves and the
//! error messages are functions in std.json (`data_i64`, `data_error`, ...).

use brim_ir::{BinaryOp as Ir, Const, InstKind, SlotId, Span, Terminator, Type as IrType, ValueId};
use brim_runtime::RuntimeFn;
use brim_semantic::{Def, EnumId, Type};

use super::{FnLowerer, Helper, ModuleLowerer};

/// Helpers have no source position of their own.
const NO_SPAN: Span = Span { start: 0, end: 0 };

/// The variants of std.json's `Json`, in declaration order.
const NULL: usize = 0;
const BOOL: usize = 1;
const NUMBER: usize = 2;
const STR: usize = 3;
const ARRAY: usize = 4;
const OBJECT: usize = 5;

impl ModuleLowerer<'_> {
    /// std.json's `Json`. Only used when std.json is part of the program.
    pub(super) fn json_type(&mut self) -> Type {
        let module = self.program.modules.iter().position(|m| m == "std.json");
        let id = self
            .program
            .enums
            .iter()
            .position(|e| e.name == "Json" && e.module.is_some() && e.module == module)
            .expect("std.json is loaded");
        self.types.intern(Def::Enum(EnumId(id)), Vec::new())
    }

    /// `Result[ty, string]`.
    pub(super) fn result_of(&mut self, ty: Type) -> Type {
        self.types
            .intern(Def::Enum(EnumId(1)), vec![ty, Type::String])
    }
}

impl FnLowerer<'_, '_> {
    /// `value` (of the concrete type `ty`) as a `Json`.
    pub(super) fn encode_value(&mut self, value: ValueId, ty: Type) -> ValueId {
        let json = self.m.json_type();
        match ty {
            Type::I64 | Type::I32 | Type::U8 | Type::U64 => {
                let rt = if ty == Type::U64 {
                    RuntimeFn::U64ToStr
                } else {
                    RuntimeFn::I64ToStr
                };
                let text =
                    self.emit_value(InstKind::CallRuntime(rt, vec![value]), IrType::Str, NO_SPAN);
                self.make_variant(json, NUMBER, &[text], NO_SPAN)
            }
            Type::F64 => self.json_fn("from_float", vec![value]),
            Type::Bool => self.make_variant(json, BOOL, &[value], NO_SPAN),
            Type::String => self.make_variant(json, STR, &[value], NO_SPAN),
            _ => {
                let helper = self.m.helper(Helper::ToJson, ty);
                let json_ir = self.m.ir_type(json);
                self.emit_value(InstKind::Call(helper, vec![value]), json_ir, NO_SPAN)
            }
        }
    }

    /// A `Result[ty, string]`: `json` read as a `ty`. `path` says where in
    /// the document `json` is, for errors.
    pub(super) fn decode_value(&mut self, json: ValueId, path: ValueId, ty: Type) -> ValueId {
        let leaf = match ty {
            Type::I64 => Some("data_i64"),
            Type::I32 => Some("data_i32"),
            Type::U8 => Some("data_u8"),
            Type::U64 => Some("data_u64"),
            Type::F64 => Some("data_f64"),
            Type::Bool => Some("data_bool"),
            Type::String => Some("data_string"),
            _ => None,
        };
        if let Some(leaf) = leaf {
            return self.json_fn(leaf, vec![json, path]);
        }
        let helper = self.m.helper(Helper::FromJson, ty);
        let result = self.m.result_of(ty);
        let result_ir = self.m.ir_type(result);
        self.emit_value(InstKind::Call(helper, vec![json, path]), result_ir, NO_SPAN)
    }

    /// Calls the free function `name` of std.json.
    pub(super) fn json_fn(&mut self, name: &str, args: Vec<ValueId>) -> ValueId {
        self.std_fn("std.json", name, args)
    }

    /// Calls the free function `name` of the standard library module
    /// `module`.
    pub(super) fn std_fn(&mut self, module: &str, name: &str, args: Vec<ValueId>) -> ValueId {
        let id = self
            .m
            .program
            .module_fn(module, name)
            .unwrap_or_else(|| panic!("{module} has `{name}`"));
        let ret = self.m.program.functions[id.0].ret;
        let ret_ir = self.m.ir_type(ret);
        let f = self.m.instance(id.0, Vec::new());
        self.emit_value(InstKind::Call(f, args), ret_ir, NO_SPAN)
    }

    fn json_list_type(&mut self) -> Type {
        let json = self.m.json_type();
        self.m.types.intern(Def::List, vec![json])
    }

    fn json_map_type(&mut self) -> Type {
        let json = self.m.json_type();
        self.m.types.intern(Def::Map, vec![Type::String, json])
    }

    /// A slot holding an empty list or map of the semantic type `ty`.
    fn empty_slot(&mut self, name: &str, ty: Type) -> SlotId {
        let ir = self.m.ir_type(ty);
        let kind = match ir {
            IrType::List(id) => InstKind::ListNew(id, Vec::new()),
            IrType::Map(id) => InstKind::MapNew(id),
            _ => unreachable!("a list or a map"),
        };
        let empty = self.emit_value(kind, ir, NO_SPAN);
        let slot = self.new_slot(name, ir);
        self.emit(InstKind::Store(slot, empty), None, NO_SPAN);
        slot
    }

    /// Adds `item` to the list in `slot`.
    fn push_to(&mut self, slot: SlotId, item: ValueId) {
        let list = self.emit_value(
            InstKind::Take(slot),
            self.func.slots[slot.index()].ty,
            NO_SPAN,
        );
        let ty = self.func.slots[slot.index()].ty;
        let longer = self.emit_value(InstKind::ListPush(list, item), ty, NO_SPAN);
        self.emit(InstKind::Store(slot, longer), None, NO_SPAN);
    }

    /// Sets `key` to `value` in the map in `slot`.
    fn insert_into(&mut self, slot: SlotId, key: ValueId, value: ValueId) {
        let ty = self.func.slots[slot.index()].ty;
        let map = self.emit_value(InstKind::Take(slot), ty, NO_SPAN);
        let bigger = self.emit_value(InstKind::MapInsert(map, key, value), ty, NO_SPAN);
        self.emit(InstKind::Store(slot, bigger), None, NO_SPAN);
    }

    /// Stores `value` in a new slot.
    pub(super) fn keep(&mut self, name: &str, value: ValueId) -> SlotId {
        let ty = self.func.values[value.index()];
        let slot = self.new_slot(name, ty);
        self.emit(InstKind::Store(slot, value), None, NO_SPAN);
        slot
    }

    /// `path + suffix`.
    fn extend_path(&mut self, path: SlotId, suffix: ValueId) -> ValueId {
        let p = self.load(path);
        self.concat(p, suffix)
    }

    // ---- to JSON --------------------------------------------------------

    pub(super) fn encode_body(&mut self, ty: Type) {
        let value = SlotId(0);
        let json = self.m.json_type();
        let (def, args) = self.type_args_of(ty);
        let result = match def {
            Def::Fn => unreachable!("functions do not implement `Data`"),
            Def::Box => {
                let x = self.boxed(value, args[0]);
                self.encode_value(x, args[0])
            }
            Def::Tuple => {
                let list_ty = self.json_list_type();
                let items = self.empty_slot("items", list_ty);
                for (i, item_ty) in args.iter().copied().enumerate() {
                    let x = self.field(value, i, item_ty);
                    let j = self.encode_value(x, item_ty);
                    self.push_to(items, j);
                }
                let items = self.load(items);
                self.make_variant(json, ARRAY, &[items], NO_SPAN)
            }
            Def::Struct(_) => {
                let map_ty = self.json_map_type();
                let members = self.empty_slot("members", map_ty);
                for (i, (name, field_ty)) in self.struct_fields(ty).into_iter().enumerate() {
                    let x = self.field(value, i, field_ty);
                    let j = self.encode_value(x, field_ty);
                    let key = self.text(&name);
                    self.insert_into(members, key, j);
                }
                let members = self.load(members);
                self.make_variant(json, OBJECT, &[members], NO_SPAN)
            }
            Def::Enum(e) => {
                self.encode_enum(ty, e == EnumId(0));
                return;
            }
            Def::List | Def::Set => {
                let is_set = def == Def::Set;
                let list_ty = self.json_list_type();
                let items = self.empty_slot("items", list_ty);
                let elem = args[0];
                let elem_ir = self.m.ir_type(elem);
                self.count_loop(
                    |s| {
                        let v = s.load(value);
                        let kind = if is_set {
                            InstKind::MapLen(v)
                        } else {
                            InstKind::ListLen(v)
                        };
                        s.emit_value(kind, IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        let (v, index) = (s.load(value), s.load(i));
                        let kind = if is_set {
                            InstKind::MapKeyAt(v, index)
                        } else {
                            InstKind::ListGet(v, index)
                        };
                        let x = s.emit_value(kind, elem_ir, NO_SPAN);
                        let j = s.encode_value(x, elem);
                        s.push_to(items, j);
                    },
                );
                let items = self.load(items);
                self.make_variant(json, ARRAY, &[items], NO_SPAN)
            }
            Def::Map => {
                let map_ty = self.json_map_type();
                let members = self.empty_slot("members", map_ty);
                let (key_ty, val_ty) = (args[0], args[1]);
                let (key_ir, val_ir) = (self.m.ir_type(key_ty), self.m.ir_type(val_ty));
                self.count_loop(
                    |s| {
                        let v = s.load(value);
                        s.emit_value(InstKind::MapLen(v), IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        let (v, index) = (s.load(value), s.load(i));
                        let k = s.emit_value(InstKind::MapKeyAt(v, index), key_ir, NO_SPAN);
                        let v = s.load(value);
                        let x = s.emit_value(InstKind::MapValueAt(v, index), val_ir, NO_SPAN);
                        let key = s.key_text(k, key_ty);
                        let j = s.encode_value(x, val_ty);
                        s.insert_into(members, key, j);
                    },
                );
                let members = self.load(members);
                self.make_variant(json, OBJECT, &[members], NO_SPAN)
            }
        };
        self.terminate(Terminator::Return(Some(result)));
    }

    /// A map key as the text of a JSON object key.
    fn key_text(&mut self, key: ValueId, ty: Type) -> ValueId {
        let rt = match ty {
            Type::String => return key,
            Type::Bool => RuntimeFn::BoolToStr,
            Type::U64 => RuntimeFn::U64ToStr,
            _ => RuntimeFn::I64ToStr,
        };
        self.emit_value(InstKind::CallRuntime(rt, vec![key]), IrType::Str, NO_SPAN)
    }

    /// One branch per variant, each returning the variant as JSON.
    fn encode_enum(&mut self, ty: Type, is_option: bool) {
        let value = SlotId(0);
        let json = self.m.json_type();
        let layout = self.m.enum_layout(ty);
        let variants = self.enum_variants(ty);
        let plain = variants.iter().all(|(_, fields)| fields.is_empty());
        for (v, (name, fields)) in variants.into_iter().enumerate() {
            let other = self.new_block();
            let tag = self.field(value, 0, Type::I64);
            let expected = self.int(v as i64);
            let is = self.int_op(Ir::IEq, tag, expected);
            self.require(is, other);
            let result = if is_option {
                // `Some(x)` is `x`; `None` is `null`.
                match fields.first() {
                    Some(&inner) => {
                        let x = self.field(value, layout.offsets[v], inner);
                        self.encode_value(x, inner)
                    }
                    None => self.make_variant(json, NULL, &[], NO_SPAN),
                }
            } else if plain {
                let text = self.text(&name);
                self.make_variant(json, STR, &[text], NO_SPAN)
            } else {
                let map_ty = self.json_map_type();
                let members = self.empty_slot("members", map_ty);
                let kind = self.text(&name);
                let kind = self.make_variant(json, STR, &[kind], NO_SPAN);
                let key = self.text("kind");
                self.insert_into(members, key, kind);
                let payload = match fields.as_slice() {
                    [] => None,
                    [one] => {
                        let x = self.field(value, layout.offsets[v], *one);
                        Some(self.encode_value(x, *one))
                    }
                    many => {
                        let list_ty = self.json_list_type();
                        let items = self.empty_slot("items", list_ty);
                        for (j, field_ty) in many.iter().copied().enumerate() {
                            let x = self.field(value, layout.offsets[v] + j, field_ty);
                            let item = self.encode_value(x, field_ty);
                            self.push_to(items, item);
                        }
                        let items = self.load(items);
                        Some(self.make_variant(json, ARRAY, &[items], NO_SPAN))
                    }
                };
                if let Some(payload) = payload {
                    let key = self.text("value");
                    self.insert_into(members, key, payload);
                }
                let members = self.load(members);
                self.make_variant(json, OBJECT, &[members], NO_SPAN)
            };
            self.terminate(Terminator::Return(Some(result)));
            self.switch_to(other);
        }
        self.terminate(Terminator::Unreachable);
    }

    // ---- from JSON ------------------------------------------------------

    /// The value of `result` (a `Result[ty, string]`); returns its error
    /// from this function (whose result is also a `Result`) if it has one.
    fn ok_or_return(&mut self, result: ValueId, ty: Type) -> ValueId {
        let result_ty = self.m.result_of(ty);
        let layout = self.m.enum_layout(result_ty);
        let slot = self.keep("result", result);
        let tag = self.field(slot, 0, Type::I64);
        let ok = self.int(0);
        let is_ok = self.int_op(Ir::IEq, tag, ok);
        let failed = self.new_block();
        let next = self.new_block();
        self.terminate(Terminator::Branch {
            cond: is_ok,
            then_block: next,
            else_block: failed,
        });
        self.switch_to(failed);
        let error = self.field(slot, layout.offsets[1], Type::String);
        let ret = self.ret;
        let err = self.make_variant(ret, 1, &[error], NO_SPAN);
        self.terminate(Terminator::Return(Some(err)));
        self.switch_to(next);
        self.field(slot, layout.offsets[0], ty)
    }

    /// Returns `Ok(value)` from this function.
    fn return_ok(&mut self, value: ValueId) {
        let ret = self.ret;
        let ok = self.make_variant(ret, 0, &[value], NO_SPAN);
        self.terminate(Terminator::Return(Some(ok)));
    }

    pub(super) fn decode_body(&mut self, ty: Type) {
        let (value, path) = (SlotId(0), SlotId(1));
        let (def, args) = self.type_args_of(ty);
        match def {
            Def::Fn => unreachable!("functions do not implement `Data`"),
            Def::Box => {
                let (j, p) = (self.load(value), self.load(path));
                let r = self.decode_value(j, p, args[0]);
                let x = self.ok_or_return(r, args[0]);
                let IrType::List(list) = self.m.ir_type(ty) else {
                    unreachable!("boxes lower to lists")
                };
                let ir = self.m.ir_type(ty);
                let boxed = self.emit_value(InstKind::ListNew(list, vec![x]), ir, NO_SPAN);
                self.return_ok(boxed);
            }
            Def::Tuple => {
                let items = self.tuple_items(value, path, args.len());
                let mut parts = Vec::with_capacity(args.len());
                for (i, item_ty) in args.iter().copied().enumerate() {
                    let x = self.decode_item(items, path, i, item_ty);
                    parts.push(self.keep("part", x));
                }
                let values: Vec<ValueId> = parts.iter().map(|s| self.load(*s)).collect();
                let ir = self.m.ir_type(ty);
                let IrType::Struct(id) = ir else {
                    unreachable!("tuples lower to structs")
                };
                let tuple = self.emit_value(InstKind::MakeStruct(id, values), ir, NO_SPAN);
                self.return_ok(tuple);
            }
            Def::Struct(_) => {
                let (j, p) = (self.load(value), self.load(path));
                let map_ty = self.json_map_type();
                let r = self.json_fn("data_object", vec![j, p]);
                let members = self.ok_or_return(r, map_ty);
                let members = self.keep("members", members);
                let mut parts = Vec::new();
                for (name, field_ty) in self.struct_fields(ty) {
                    let m = self.load(members);
                    let key = self.text(&name);
                    let member = self.json_fn("data_member", vec![m, key]);
                    let suffix = self.text(&format!(".{name}"));
                    let p = self.extend_path(path, suffix);
                    let r = self.decode_value(member, p, field_ty);
                    let x = self.ok_or_return(r, field_ty);
                    parts.push(self.keep("field", x));
                }
                let values: Vec<ValueId> = parts.iter().map(|s| self.load(*s)).collect();
                let ir = self.m.ir_type(ty);
                let IrType::Struct(id) = ir else {
                    unreachable!("structs lower to structs")
                };
                let made = self.emit_value(InstKind::MakeStruct(id, values), ir, NO_SPAN);
                self.return_ok(made);
            }
            Def::Enum(EnumId(0)) => {
                // `null` is `None`; anything else the value inside `Some`.
                let tag = self.field(value, 0, Type::I64);
                let null = self.int(NULL as i64);
                let is_null = self.int_op(Ir::IEq, tag, null);
                let none_block = self.new_block();
                let some_block = self.new_block();
                self.terminate(Terminator::Branch {
                    cond: is_null,
                    then_block: none_block,
                    else_block: some_block,
                });
                self.switch_to(none_block);
                let none = self.make_variant(ty, 1, &[], NO_SPAN);
                self.return_ok(none);
                self.switch_to(some_block);
                let (j, p) = (self.load(value), self.load(path));
                let r = self.decode_value(j, p, args[0]);
                let x = self.ok_or_return(r, args[0]);
                let some = self.make_variant(ty, 0, &[x], NO_SPAN);
                self.return_ok(some);
            }
            Def::Enum(e) => self.decode_enum(ty, e),
            Def::List | Def::Set => {
                let is_set = def == Def::Set;
                let elem = args[0];
                let (j, p) = (self.load(value), self.load(path));
                let list_ty = self.json_list_type();
                let r = self.json_fn("data_array", vec![j, p]);
                let items = self.ok_or_return(r, list_ty);
                let items = self.keep("items", items);
                let out = self.empty_slot("out", ty);
                self.count_loop(
                    |s| {
                        let v = s.load(items);
                        s.emit_value(InstKind::ListLen(v), IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        let index = s.load(i);
                        let x = s.decode_indexed(items, path, index, elem);
                        if is_set {
                            let yes = s.emit_value(
                                InstKind::Const(Const::Bool(true)),
                                IrType::Bool,
                                NO_SPAN,
                            );
                            s.insert_into(out, x, yes);
                        } else {
                            s.push_to(out, x);
                        }
                    },
                );
                let out = self.load(out);
                self.return_ok(out);
            }
            Def::Map => {
                let (key_ty, val_ty) = (args[0], args[1]);
                let (j, p) = (self.load(value), self.load(path));
                let map_ty = self.json_map_type();
                let r = self.json_fn("data_object", vec![j, p]);
                let members = self.ok_or_return(r, map_ty);
                let members = self.keep("members", members);
                let json_ir = {
                    let json = self.m.json_type();
                    self.m.ir_type(json)
                };
                let out = self.empty_slot("out", ty);
                self.count_loop(
                    |s| {
                        let v = s.load(members);
                        s.emit_value(InstKind::MapLen(v), IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        let (m, index) = (s.load(members), s.load(i));
                        let key = s.emit_value(InstKind::MapKeyAt(m, index), IrType::Str, NO_SPAN);
                        let key = s.keep("key", key);
                        let k = s.load(key);
                        let dot = s.text(".");
                        let suffix = s.concat(dot, k);
                        let p = s.extend_path(path, suffix);
                        let p = s.keep("member_path", p);
                        let k = if key_ty == Type::String {
                            s.load(key)
                        } else {
                            let text = s.load(key);
                            let kj = s.json_fn("data_key", vec![text]);
                            let p = s.load(p);
                            let r = s.decode_value(kj, p, key_ty);
                            s.ok_or_return(r, key_ty)
                        };
                        let k = s.keep("map_key", k);
                        let (m, index) = (s.load(members), s.load(i));
                        let x = s.emit_value(InstKind::MapValueAt(m, index), json_ir, NO_SPAN);
                        let p = s.load(p);
                        let r = s.decode_value(x, p, val_ty);
                        let v = s.ok_or_return(r, val_ty);
                        let k = s.load(k);
                        s.insert_into(out, k, v);
                    },
                );
                let out = self.load(out);
                self.return_ok(out);
            }
        }
    }

    /// The array of exactly `n` values in the `Json` in `value`, in a slot.
    fn tuple_items(&mut self, value: SlotId, path: SlotId, n: usize) -> SlotId {
        let (j, p) = (self.load(value), self.load(path));
        let n = self.int(n as i64);
        let list_ty = self.json_list_type();
        let r = self.json_fn("data_tuple", vec![j, p, n]);
        let items = self.ok_or_return(r, list_ty);
        self.keep("items", items)
    }

    /// Item `i` of the JSON array in `items`, read as `ty`.
    fn decode_item(&mut self, items: SlotId, path: SlotId, i: usize, ty: Type) -> ValueId {
        let index = self.int(i as i64);
        self.decode_indexed(items, path, index, ty)
    }

    /// The item at `index` of the JSON array in `items`, read as `ty`; its
    /// path is `path[index]`.
    fn decode_indexed(&mut self, items: SlotId, path: SlotId, index: ValueId, ty: Type) -> ValueId {
        let json = self.m.json_type();
        let json_ir = self.m.ir_type(json);
        let list = self.load(items);
        let item = self.emit_value(InstKind::ListGet(list, index), json_ir, NO_SPAN);
        let open = self.text("[");
        let number = self.emit_value(
            InstKind::CallRuntime(RuntimeFn::I64ToStr, vec![index]),
            IrType::Str,
            NO_SPAN,
        );
        let close = self.text("]");
        let suffix = self.concat(open, number);
        let suffix = self.concat(suffix, close);
        let p = self.extend_path(path, suffix);
        let r = self.decode_value(item, p, ty);
        self.ok_or_return(r, ty)
    }

    /// An enum other than `Option`: `"Name"` or `{"kind": "Name", "value": ...}`.
    fn decode_enum(&mut self, ty: Type, e: EnumId) {
        let (value, path) = (SlotId(0), SlotId(1));
        let type_name = self.m.program.enum_def(e).name.clone();
        let (j, p) = (self.load(value), self.load(path));
        let name = self.text(&type_name);
        let r = self.json_fn("data_kind", vec![j, p, name]);
        let kind = self.ok_or_return(r, Type::String);
        let kind = self.keep("kind", kind);
        for (v, (variant, fields)) in self.enum_variants(ty).into_iter().enumerate() {
            let other = self.new_block();
            let k = self.load(kind);
            let expected = self.text(&variant);
            let is = self.emit_value(
                InstKind::CallRuntime(RuntimeFn::StrEq, vec![k, expected]),
                IrType::Bool,
                NO_SPAN,
            );
            self.require(is, other);
            let mut parts = Vec::new();
            if !fields.is_empty() {
                let j = self.load(value);
                let payload = self.json_fn("data_payload", vec![j]);
                let payload = self.keep("payload", payload);
                let suffix = self.text(".value");
                let p = self.extend_path(path, suffix);
                let p = self.keep("payload_path", p);
                if let [one] = fields.as_slice() {
                    let (j, p) = (self.load(payload), self.load(p));
                    let r = self.decode_value(j, p, *one);
                    let x = self.ok_or_return(r, *one);
                    parts.push(self.keep("part", x));
                } else {
                    let items = self.tuple_items(payload, p, fields.len());
                    for (i, field_ty) in fields.iter().copied().enumerate() {
                        let x = self.decode_item(items, p, i, field_ty);
                        parts.push(self.keep("part", x));
                    }
                }
            }
            let values: Vec<ValueId> = parts.iter().map(|s| self.load(*s)).collect();
            let made = self.make_variant(ty, v, &values, NO_SPAN);
            self.return_ok(made);
            self.switch_to(other);
        }
        let (p, k) = (self.load(path), self.load(kind));
        let name = self.text(&type_name);
        let message = self.json_fn("data_unknown_variant", vec![p, k, name]);
        let ret = self.ret;
        let err = self.make_variant(ret, 1, &[message], NO_SPAN);
        self.terminate(Terminator::Return(Some(err)));
    }
}
