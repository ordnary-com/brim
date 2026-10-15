//! Functions generated per concrete type: `==` on structs, enums, lists and
//! maps, and their text form for `print` and `to_string`.
//!
//! Each is generated once per type, the first time it is needed, and calls
//! the helpers of the types inside it, so recursive types work.

use brim_ir::{
    BlockId, Const, FuncId, Function, InstKind, Slot, SlotId, Span, Terminator, Type as IrType,
    ValueId,
};
use brim_runtime::RuntimeFn;
use brim_semantic::{Def, Type};

use super::{FnLowerer, Helper, Job, ModuleLowerer};

impl ModuleLowerer<'_> {
    /// The generated function `helper` for the concrete type `ty`, queued
    /// for lowering the first time it is requested.
    pub(super) fn helper(&mut self, helper: Helper, ty: Type) -> FuncId {
        if let Some(id) = self.helpers.get(&(helper, ty)) {
            return *id;
        }
        let id = super::func_id(self.module.functions.len());
        let what = match helper {
            Helper::Eq => "==",
            Helper::Show => "to_string",
        };
        self.module.functions.push(Function {
            name: format!("{what}[{}]", self.type_name(ty)),
            consumes_self: false,
            param_count: 0,
            ret: IrType::Void,
            slots: Vec::new(),
            values: Vec::new(),
            blocks: Vec::new(),
        });
        self.helpers.insert((helper, ty), id);
        self.queue.push_back(Job::Helper(id, helper, ty));
        id
    }
}

/// Helpers have no source position of their own.
const NO_SPAN: Span = Span { start: 0, end: 0 };

impl<'m, 'p> FnLowerer<'m, 'p> {
    pub(super) fn helper(m: &'m mut ModuleLowerer<'p>, helper: Helper, ty: Type) -> Function {
        let ir_ty = m.ir_type(ty);
        let (params, ret, ret_ir): (&[&str], _, _) = match helper {
            Helper::Eq => (&["a", "b"], Type::Bool, IrType::Bool),
            Helper::Show => (&["value"], Type::String, IrType::Str),
        };
        let mut lowerer = FnLowerer {
            m,
            type_args: Vec::new(),
            ret,
            self_return: None,
            func: Function {
                consumes_self: false,
                name: String::new(),
                param_count: params.len(),
                ret: ret_ir,
                slots: params
                    .iter()
                    .map(|name| Slot {
                        name: (*name).to_string(),
                        ty: ir_ty,
                    })
                    .collect(),
                values: Vec::new(),
                blocks: Vec::new(),
            },
            current: BlockId(0),
            terminated: false,
            loops: Vec::new(),
        };
        lowerer.current = lowerer.new_block();
        match helper {
            Helper::Eq => lowerer.eq_body(ty),
            Helper::Show => lowerer.show_body(ty),
        }
        lowerer.func
    }

    /// `a == b` for two values of the concrete type `ty`.
    pub(super) fn values_equal(&mut self, a: ValueId, b: ValueId, ty: Type, span: Span) -> ValueId {
        use brim_ir::BinaryOp as Ir;
        let op = match ty {
            Type::I64 | Type::I32 | Type::U8 | Type::U64 => Ir::IEq,
            Type::Bool => Ir::BEq,
            Type::F64 => Ir::FEq,
            Type::String => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrEq, vec![a, b]);
                return self.emit_value(kind, IrType::Bool, span);
            }
            _ => {
                let helper = self.m.helper(Helper::Eq, ty);
                return self.emit_value(InstKind::Call(helper, vec![a, b]), IrType::Bool, span);
            }
        };
        self.emit_value(InstKind::Binary(op, a, b), IrType::Bool, span)
    }

    /// The text form of a value of the concrete type `ty`. Strings inside
    /// other values are shown quoted.
    pub(super) fn value_text(&mut self, v: ValueId, ty: Type, span: Span) -> ValueId {
        let runtime = match ty {
            Type::I64 | Type::I32 | Type::U8 => RuntimeFn::I64ToStr,
            Type::U64 => RuntimeFn::U64ToStr,
            Type::Bool => RuntimeFn::BoolToStr,
            Type::F64 => RuntimeFn::F64ToStr,
            Type::String => RuntimeFn::StrQuote,
            _ => {
                let helper = self.m.helper(Helper::Show, ty);
                return self.emit_value(InstKind::Call(helper, vec![v]), IrType::Str, span);
            }
        };
        self.emit_value(InstKind::CallRuntime(runtime, vec![v]), IrType::Str, span)
    }

    // ---- small pieces -----------------------------------------------------

    fn load(&mut self, slot: SlotId) -> ValueId {
        let ty = self.func.slots[slot.index()].ty;
        self.emit_value(InstKind::Load(slot), ty, NO_SPAN)
    }

    fn int(&mut self, v: i64) -> ValueId {
        self.emit_value(InstKind::Const(Const::I64(v)), IrType::I64, NO_SPAN)
    }

    fn text(&mut self, s: &str) -> ValueId {
        self.emit_value(
            InstKind::Const(Const::Str(s.to_string())),
            IrType::Str,
            NO_SPAN,
        )
    }

    fn concat(&mut self, a: ValueId, b: ValueId) -> ValueId {
        let kind = InstKind::CallRuntime(RuntimeFn::StrConcat, vec![a, b]);
        self.emit_value(kind, IrType::Str, NO_SPAN)
    }

    fn int_op(&mut self, op: brim_ir::BinaryOp, a: ValueId, b: ValueId) -> ValueId {
        let ty = op.signature().1;
        self.emit_value(InstKind::Binary(op, a, b), ty, NO_SPAN)
    }

    /// Continues in a new block when `cond` holds, else jumps to `otherwise`.
    fn require(&mut self, cond: ValueId, otherwise: BlockId) {
        let next = self.new_block();
        self.terminate(Terminator::Branch {
            cond,
            then_block: next,
            else_block: otherwise,
        });
        self.switch_to(next);
    }

    /// Field `index` of the value in `slot`.
    fn field(&mut self, slot: SlotId, index: usize, ty: Type) -> ValueId {
        let v = self.load(slot);
        let ir_ty = self.m.ir_type(ty);
        self.emit_value(InstKind::Extract(v, index), ir_ty, NO_SPAN)
    }

    /// The value in the box in `slot`.
    fn boxed(&mut self, slot: SlotId, inner: Type) -> ValueId {
        let b = self.load(slot);
        let zero = self.int(0);
        let ir_ty = self.m.ir_type(inner);
        self.emit_value(InstKind::ListGet(b, zero), ir_ty, NO_SPAN)
    }

    /// The concrete types of the fields of a struct, or of each variant's
    /// payload of an enum.
    fn struct_fields(&mut self, ty: Type) -> Vec<(String, Type)> {
        let Type::Named(id) = ty else {
            unreachable!("structs are named")
        };
        let named = self.m.types.get(id).clone();
        let Def::Struct(s) = named.def else {
            unreachable!("a struct")
        };
        let fields = self.m.program.struct_def(s).fields.clone();
        fields
            .into_iter()
            .map(|f| (f.name, self.m.types.subst(f.ty, &named.args)))
            .collect()
    }

    fn enum_variants(&mut self, ty: Type) -> Vec<(String, Vec<Type>)> {
        let Type::Named(id) = ty else {
            unreachable!("enums are named")
        };
        let named = self.m.types.get(id).clone();
        let Def::Enum(e) = named.def else {
            unreachable!("an enum")
        };
        let variants = self.m.program.enum_def(e).variants.clone();
        variants
            .into_iter()
            .map(|v| {
                let fields = v
                    .fields
                    .iter()
                    .map(|f| self.m.types.subst(*f, &named.args))
                    .collect();
                (v.name, fields)
            })
            .collect()
    }

    /// The element type of a list, or the key and value types of a map.
    fn type_args_of(&self, ty: Type) -> (Def, Vec<Type>) {
        let Type::Named(id) = ty else {
            unreachable!("a named type")
        };
        let named = self.m.types.get(id);
        (named.def, named.args.clone())
    }

    /// A loop over `0..len`: calls `body` with the index slot in a new block
    /// and continues after the loop.
    fn count_loop(
        &mut self,
        len: impl Fn(&mut Self) -> ValueId,
        body: impl FnOnce(&mut Self, SlotId),
    ) {
        use brim_ir::BinaryOp as Ir;
        let i = self.new_slot("i", IrType::I64);
        let zero = self.int(0);
        self.emit(InstKind::Store(i, zero), None, NO_SPAN);
        let header = self.new_block();
        let exit = self.new_block();
        self.terminate(Terminator::Jump(header));

        self.switch_to(header);
        let index = self.load(i);
        let n = len(self);
        let more = self.int_op(Ir::ILt, index, n);
        self.require(more, exit);
        body(self, i);
        let index = self.load(i);
        let one = self.int(1);
        let next = self.int_op(Ir::IAdd, index, one);
        self.emit(InstKind::Store(i, next), None, NO_SPAN);
        self.terminate(Terminator::Jump(header));

        self.switch_to(exit);
    }

    fn return_bool(&mut self, value: bool) {
        let v = self.emit_value(InstKind::Const(Const::Bool(value)), IrType::Bool, NO_SPAN);
        self.terminate(Terminator::Return(Some(v)));
    }

    // ---- `==` ---------------------------------------------------------------

    fn eq_body(&mut self, ty: Type) {
        use brim_ir::BinaryOp as Ir;
        let (a, b) = (SlotId(0), SlotId(1));
        let differ = self.new_block();
        let (def, args) = self.type_args_of(ty);
        match def {
            Def::Fn => unreachable!("functions cannot be compared"),
            Def::Tuple => {
                for (index, item_ty) in args.iter().copied().enumerate() {
                    let x = self.field(a, index, item_ty);
                    let y = self.field(b, index, item_ty);
                    let eq = self.values_equal(x, y, item_ty, NO_SPAN);
                    self.require(eq, differ);
                }
            }
            Def::Box => {
                let x = self.boxed(a, args[0]);
                let y = self.boxed(b, args[0]);
                let eq = self.values_equal(x, y, args[0], NO_SPAN);
                self.require(eq, differ);
            }
            Def::Struct(_) => {
                for (index, (_, field_ty)) in self.struct_fields(ty).into_iter().enumerate() {
                    let x = self.field(a, index, field_ty);
                    let y = self.field(b, index, field_ty);
                    let eq = self.values_equal(x, y, field_ty, NO_SPAN);
                    self.require(eq, differ);
                }
            }
            Def::Enum(_) => {
                let layout = self.m.enum_layout(ty);
                let tag_a = self.field(a, 0, Type::I64);
                let tag_b = self.field(b, 0, Type::I64);
                let same = self.int_op(Ir::IEq, tag_a, tag_b);
                self.require(same, differ);
                let equal = self.new_block();
                for (v, (_, fields)) in self.enum_variants(ty).into_iter().enumerate() {
                    if fields.is_empty() {
                        continue;
                    }
                    let other = self.new_block();
                    let tag = self.field(a, 0, Type::I64);
                    let expected = self.int(v as i64);
                    let is = self.int_op(Ir::IEq, tag, expected);
                    self.require(is, other);
                    for (j, field_ty) in fields.into_iter().enumerate() {
                        let index = layout.offsets[v] + j;
                        let x = self.field(a, index, field_ty);
                        let y = self.field(b, index, field_ty);
                        let eq = self.values_equal(x, y, field_ty, NO_SPAN);
                        self.require(eq, differ);
                    }
                    self.terminate(Terminator::Jump(equal));
                    self.switch_to(other);
                }
                self.terminate(Terminator::Jump(equal));
                self.switch_to(equal);
            }
            Def::List => {
                let elem = args[0];
                let elem_ir = self.m.ir_type(elem);
                let la = self.load(a);
                let len_a = self.emit_value(InstKind::ListLen(la), IrType::I64, NO_SPAN);
                let lb = self.load(b);
                let len_b = self.emit_value(InstKind::ListLen(lb), IrType::I64, NO_SPAN);
                let same = self.int_op(Ir::IEq, len_a, len_b);
                self.require(same, differ);
                self.count_loop(
                    |s| {
                        let l = s.load(a);
                        s.emit_value(InstKind::ListLen(l), IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        let (la, index) = (s.load(a), s.load(i));
                        let x = s.emit_value(InstKind::ListGet(la, index), elem_ir, NO_SPAN);
                        let lb = s.load(b);
                        let y = s.emit_value(InstKind::ListGet(lb, index), elem_ir, NO_SPAN);
                        let eq = s.values_equal(x, y, elem, NO_SPAN);
                        s.require(eq, differ);
                    },
                );
            }
            // A set is a map to `true`: equal when the keys are.
            Def::Map | Def::Set => {
                let (key, value) = (args[0], args.get(1).copied().unwrap_or(Type::Bool));
                let (key_ir, value_ir) = (self.m.ir_type(key), self.m.ir_type(value));
                let ma = self.load(a);
                let len_a = self.emit_value(InstKind::MapLen(ma), IrType::I64, NO_SPAN);
                let mb = self.load(b);
                let len_b = self.emit_value(InstKind::MapLen(mb), IrType::I64, NO_SPAN);
                let same = self.int_op(Ir::IEq, len_a, len_b);
                self.require(same, differ);
                let position = self.new_slot("position", IrType::I64);
                self.count_loop(
                    |s| {
                        let m = s.load(a);
                        s.emit_value(InstKind::MapLen(m), IrType::I64, NO_SPAN)
                    },
                    |s, i| {
                        // Every key of `a` is in `b`, with an equal value.
                        let (ma, index) = (s.load(a), s.load(i));
                        let k = s.emit_value(InstKind::MapKeyAt(ma, index), key_ir, NO_SPAN);
                        let mb = s.load(b);
                        let found = s.emit_value(InstKind::MapFind(mb, k), IrType::I64, NO_SPAN);
                        s.emit(InstKind::Store(position, found), None, NO_SPAN);
                        let missing = s.int(-1);
                        let has = s.int_op(Ir::INe, found, missing);
                        s.require(has, differ);
                        let (ma, index) = (s.load(a), s.load(i));
                        let x = s.emit_value(InstKind::MapValueAt(ma, index), value_ir, NO_SPAN);
                        let (mb, at) = (s.load(b), s.load(position));
                        let y = s.emit_value(InstKind::MapValueAt(mb, at), value_ir, NO_SPAN);
                        let eq = s.values_equal(x, y, value, NO_SPAN);
                        s.require(eq, differ);
                    },
                );
            }
        }
        self.return_bool(true);
        self.switch_to(differ);
        self.return_bool(false);
    }

    // ---- text ---------------------------------------------------------------

    fn show_body(&mut self, ty: Type) {
        use brim_ir::BinaryOp as Ir;
        let value = SlotId(0);
        let (def, args) = self.type_args_of(ty);
        match def {
            Def::Fn => unreachable!("functions have no text form"),
            Def::Tuple => {
                let mut acc = self.text("(");
                for (index, item_ty) in args.iter().copied().enumerate() {
                    if index > 0 {
                        let sep = self.text(", ");
                        acc = self.concat(acc, sep);
                    }
                    let x = self.field(value, index, item_ty);
                    let shown = self.value_text(x, item_ty, NO_SPAN);
                    acc = self.concat(acc, shown);
                }
                let close = self.text(")");
                let result = self.concat(acc, close);
                self.terminate(Terminator::Return(Some(result)));
            }
            // A box shows as the value inside it.
            Def::Box => {
                let x = self.boxed(value, args[0]);
                let shown = self.value_text(x, args[0], NO_SPAN);
                self.terminate(Terminator::Return(Some(shown)));
            }
            Def::Struct(s) => {
                let name = self.m.program.struct_def(s).name.clone();
                let fields = self.struct_fields(ty);
                if fields.is_empty() {
                    let t = self.text(&format!("{name} {{}}"));
                    self.terminate(Terminator::Return(Some(t)));
                    return;
                }
                let mut acc = self.text(&format!("{name} {{ "));
                for (index, (field_name, field_ty)) in fields.into_iter().enumerate() {
                    let sep = if index == 0 { "" } else { ", " };
                    let label = self.text(&format!("{sep}{field_name}: "));
                    acc = self.concat(acc, label);
                    let x = self.field(value, index, field_ty);
                    let shown = self.value_text(x, field_ty, NO_SPAN);
                    acc = self.concat(acc, shown);
                }
                let close = self.text(" }");
                let result = self.concat(acc, close);
                self.terminate(Terminator::Return(Some(result)));
            }
            Def::Enum(_) => {
                let layout = self.m.enum_layout(ty);
                for (v, (name, fields)) in self.enum_variants(ty).into_iter().enumerate() {
                    let other = self.new_block();
                    let tag = self.field(value, 0, Type::I64);
                    let expected = self.int(v as i64);
                    let is = self.int_op(Ir::IEq, tag, expected);
                    self.require(is, other);
                    let mut acc = self.text(&name);
                    if !fields.is_empty() {
                        let open = self.text("(");
                        acc = self.concat(acc, open);
                        for (j, field_ty) in fields.into_iter().enumerate() {
                            if j > 0 {
                                let sep = self.text(", ");
                                acc = self.concat(acc, sep);
                            }
                            let x = self.field(value, layout.offsets[v] + j, field_ty);
                            let shown = self.value_text(x, field_ty, NO_SPAN);
                            acc = self.concat(acc, shown);
                        }
                        let close = self.text(")");
                        acc = self.concat(acc, close);
                    }
                    self.terminate(Terminator::Return(Some(acc)));
                    self.switch_to(other);
                }
                self.terminate(Terminator::Unreachable);
            }
            Def::List | Def::Map | Def::Set => {
                // A set shows its elements in braces: `{1, 2}`.
                let is_set = def == Def::Set;
                let is_map = def == Def::Map || is_set;
                let acc = self.new_slot("text", IrType::Str);
                let sep = self.new_slot("separator", IrType::Str);
                let open = self.text(if is_set { "{" } else { "[" });
                self.emit(InstKind::Store(acc, open), None, NO_SPAN);
                let nothing = self.text("");
                self.emit(InstKind::Store(sep, nothing), None, NO_SPAN);
                let len = |s: &mut Self| {
                    let v = s.load(value);
                    let kind = if is_map {
                        InstKind::MapLen(v)
                    } else {
                        InstKind::ListLen(v)
                    };
                    s.emit_value(kind, IrType::I64, NO_SPAN)
                };
                if is_map && !is_set {
                    // `[:]`, as an empty map is written.
                    let not_empty = self.new_block();
                    let n = len(self);
                    let zero = self.int(0);
                    let empty = self.int_op(Ir::IEq, n, zero);
                    let empty_block = self.new_block();
                    self.terminate(Terminator::Branch {
                        cond: empty,
                        then_block: empty_block,
                        else_block: not_empty,
                    });
                    self.switch_to(empty_block);
                    let t = self.text("[:]");
                    self.terminate(Terminator::Return(Some(t)));
                    self.switch_to(not_empty);
                }
                self.count_loop(len, |s, i| {
                    let (v, index) = (s.load(value), s.load(i));
                    let item = if is_set {
                        let key_ir = s.m.ir_type(args[0]);
                        let k = s.emit_value(InstKind::MapKeyAt(v, index), key_ir, NO_SPAN);
                        s.value_text(k, args[0], NO_SPAN)
                    } else if is_map {
                        let (key, val) = (args[0], args[1]);
                        let (key_ir, val_ir) = (s.m.ir_type(key), s.m.ir_type(val));
                        let k = s.emit_value(InstKind::MapKeyAt(v, index), key_ir, NO_SPAN);
                        let x = s.emit_value(InstKind::MapValueAt(v, index), val_ir, NO_SPAN);
                        let k = s.value_text(k, key, NO_SPAN);
                        let colon = s.text(": ");
                        let k = s.concat(k, colon);
                        let x = s.value_text(x, val, NO_SPAN);
                        s.concat(k, x)
                    } else {
                        let elem_ir = s.m.ir_type(args[0]);
                        let x = s.emit_value(InstKind::ListGet(v, index), elem_ir, NO_SPAN);
                        s.value_text(x, args[0], NO_SPAN)
                    };
                    let (before, separator) = (s.load(acc), s.load(sep));
                    let with_sep = s.concat(before, separator);
                    let joined = s.concat(with_sep, item);
                    s.emit(InstKind::Store(acc, joined), None, NO_SPAN);
                    let comma = s.text(", ");
                    s.emit(InstKind::Store(sep, comma), None, NO_SPAN);
                });
                let before = self.load(acc);
                let close = self.text(if is_set { "}" } else { "]" });
                let result = self.concat(before, close);
                self.terminate(Terminator::Return(Some(result)));
            }
        }
    }
}

impl<'m, 'p> FnLowerer<'m, 'p> {
    /// The body of a thunk: calls the source function `function` with the
    /// parameters after the environment.
    pub(super) fn thunk(m: &'m mut ModuleLowerer<'p>, function: usize) -> Function {
        let f = &m.program.functions[function];
        let param_types: Vec<Type> = f.locals[..f.param_count].iter().map(|l| l.ty).collect();
        let ret = f.ret;
        let target = m.instance(function, Vec::new());
        let (_, env) = m.environment(Vec::new());
        let mut slots = vec![Slot {
            name: "<environment>".into(),
            ty: IrType::List(env),
        }];
        for (i, ty) in param_types.iter().enumerate() {
            let ty = m.ir_type(*ty);
            slots.push(Slot {
                name: format!("p{i}"),
                ty,
            });
        }
        let ret_ir = m.ir_type(ret);
        let mut lowerer = FnLowerer {
            m,
            type_args: Vec::new(),
            ret,
            self_return: None,
            func: Function {
                consumes_self: false,
                name: String::new(),
                param_count: slots.len(),
                ret: ret_ir,
                slots,
                values: Vec::new(),
                blocks: Vec::new(),
            },
            current: BlockId(0),
            terminated: false,
            loops: Vec::new(),
        };
        lowerer.current = lowerer.new_block();
        let args: Vec<ValueId> = (1..lowerer.func.param_count)
            .map(|i| lowerer.load(SlotId(i as u32)))
            .collect();
        let result = lowerer.emit(InstKind::Call(target, args), Some(ret_ir), NO_SPAN);
        lowerer.terminate(Terminator::Return(result));
        lowerer.func
    }
}
