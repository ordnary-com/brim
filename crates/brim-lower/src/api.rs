//! `serve_api(module, request)`: std.http's `api_serve` with a generated
//! function that calls the module's API functions by name.
//!
//! The generated function takes the function's name and its parameters (a
//! `Map[string, Json]`) and gives std.http's `ApiOutcome`: the result as
//! JSON, the value of an `Err` the function returned, no such function, or
//! parameters that do not fit.

use brim_ir::{
    BinaryOp as Ir, BlockId, Function, InstKind, Slot, SlotId, Span, Terminator, Type as IrType,
    ValueId,
};
use brim_runtime::RuntimeFn;
use brim_semantic::{Def, EnumId, Type};

use super::{FnLowerer, Job, ModuleLowerer};

/// Generated functions have no source position of their own.
const NO_SPAN: Span = Span { start: 0, end: 0 };

/// The variants of std.http's `ApiOutcome`, in declaration order.
const DONE: usize = 0;
const FAILED: usize = 1;
const UNKNOWN: usize = 2;
const BAD_REQUEST: usize = 3;

impl ModuleLowerer<'_> {
    /// The dispatch function of `module`, queued for lowering the first
    /// time it is requested.
    fn api_dispatcher(&mut self, module: usize) -> brim_ir::FuncId {
        if let Some(id) = self.api_dispatchers.get(&module) {
            return *id;
        }
        let id = super::func_id(self.module.functions.len());
        self.module.functions.push(Function {
            name: format!("serve_api[{}]", self.program.modules[module]),
            consumes_self: false,
            param_count: 0,
            ret: IrType::Void,
            slots: Vec::new(),
            values: Vec::new(),
            blocks: Vec::new(),
        });
        self.api_dispatchers.insert(module, id);
        self.queue.push_back(Job::ApiDispatch(id, module));
        id
    }

    /// std.http's `ApiOutcome`.
    fn api_outcome_type(&mut self) -> Type {
        let module = self.program.modules.iter().position(|m| m == "std.http");
        let id = self
            .program
            .enums
            .iter()
            .position(|e| e.name == "ApiOutcome" && e.module.is_some() && e.module == module)
            .expect("std.http is loaded");
        self.types.intern(Def::Enum(EnumId(id)), Vec::new())
    }
}

impl<'m, 'p> FnLowerer<'m, 'p> {
    /// `serve_api(module, request)`.
    pub(super) fn serve_api(&mut self, module: usize, request: ValueId, span: Span) -> ValueId {
        let dispatch = self.m.api_dispatcher(module);
        let closure = self.emit_value(InstKind::MakeClosure(dispatch, None), IrType::Closure, span);
        self.std_fn("std.http", "api_serve", vec![request, closure])
    }

    /// The body of `module`'s dispatch function: `fn(<environment>, name:
    /// str, params: Map[string, Json]) -> ApiOutcome`.
    pub(super) fn api_dispatch(m: &'m mut ModuleLowerer<'p>, module: usize) -> Function {
        let (_, env) = m.environment(Vec::new());
        let json = m.json_type();
        let params_ty = m.types.intern(Def::Map, vec![Type::String, json]);
        let params_ir = m.ir_type(params_ty);
        let outcome = m.api_outcome_type();
        let outcome_ir = m.ir_type(outcome);
        let mut lowerer = FnLowerer {
            m,
            type_args: Vec::new(),
            ret: outcome,
            self_return: None,
            func: Function {
                consumes_self: false,
                name: String::new(),
                param_count: 3,
                ret: outcome_ir,
                slots: vec![
                    Slot {
                        name: "<environment>".into(),
                        ty: IrType::List(env),
                    },
                    Slot {
                        name: "name".into(),
                        ty: IrType::Str,
                    },
                    Slot {
                        name: "params".into(),
                        ty: params_ir,
                    },
                ],
                values: Vec::new(),
                blocks: Vec::new(),
            },
            current: BlockId(0),
            terminated: false,
            loops: Vec::new(),
        };
        lowerer.current = lowerer.new_block();
        let functions: Vec<_> = lowerer
            .m
            .program
            .api
            .iter()
            .filter(|f| f.module == module)
            .cloned()
            .collect();
        for f in functions {
            let other = lowerer.new_block();
            let name = lowerer.load(SlotId(1));
            let expected = lowerer.text(&f.name);
            let is = lowerer.emit_value(
                InstKind::CallRuntime(RuntimeFn::StrEq, vec![name, expected]),
                IrType::Bool,
                NO_SPAN,
            );
            lowerer.require(is, other);
            lowerer.call_api_function(&f);
            lowerer.switch_to(other);
        }
        let unknown = lowerer.make_variant(outcome, UNKNOWN, &[], NO_SPAN);
        lowerer.terminate(Terminator::Return(Some(unknown)));
        lowerer.func
    }

    /// Reads the parameters of `f`, calls it and returns its outcome.
    fn call_api_function(&mut self, f: &brim_semantic::checked::ApiFn) {
        let outcome = self.ret;
        let mut args = Vec::with_capacity(f.params.len());
        for (name, ty) in &f.params {
            let params = self.load(SlotId(2));
            let key = self.text(name);
            let member = self.json_fn("data_member", vec![params, key]);
            let path = self.text(&format!("$.{name}"));
            let result = self.decode_value(member, path, *ty);
            let x = self.ok_or_outcome(result, *ty, BAD_REQUEST, None);
            args.push(self.keep("arg", x));
        }
        let values: Vec<ValueId> = args.iter().map(|s| self.load(*s)).collect();
        let function = self.m.instance(f.function.0, Vec::new());
        let json = self.m.json_type();
        if f.ret == Type::Void {
            self.emit(InstKind::Call(function, values), None, NO_SPAN);
            let null = self.make_variant(json, 0, &[], NO_SPAN);
            let done = self.make_variant(outcome, DONE, &[null], NO_SPAN);
            self.terminate(Terminator::Return(Some(done)));
            return;
        }
        let ret_ir = self.m.ir_type(f.ret);
        let result = self.emit_value(InstKind::Call(function, values), ret_ir, NO_SPAN);
        let (def, type_args) = self.type_args_of_any(f.ret);
        let value = if def == Some(Def::Enum(EnumId(1))) {
            // `Ok(value)` is the result; `Err(error)` is a failure.
            self.ok_or_outcome(result, type_args[0], FAILED, Some(type_args[1]))
        } else {
            result
        };
        let value_ty = if def == Some(Def::Enum(EnumId(1))) {
            type_args[0]
        } else {
            f.ret
        };
        let encoded = self.encode_value(value, value_ty);
        let done = self.make_variant(outcome, DONE, &[encoded], NO_SPAN);
        self.terminate(Terminator::Return(Some(done)));
    }

    /// The `Ok` value of `result` (a `Result[ty, E]`). For an `Err` this
    /// returns the outcome `variant` with the error: as text when
    /// `error_ty` is `None` (a decoding error), else encoded as JSON.
    fn ok_or_outcome(
        &mut self,
        result: ValueId,
        ty: Type,
        variant: usize,
        error_ty: Option<Type>,
    ) -> ValueId {
        let result_ty = match error_ty {
            Some(error) => self.m.types.intern(Def::Enum(EnumId(1)), vec![ty, error]),
            None => self.m.result_of(ty),
        };
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
        let outcome = self.ret;
        let error = self.field(slot, layout.offsets[1], error_ty.unwrap_or(Type::String));
        let payload = match error_ty {
            Some(error_ty) => self.encode_value(error, error_ty),
            None => error,
        };
        let out = self.make_variant(outcome, variant, &[payload], NO_SPAN);
        self.terminate(Terminator::Return(Some(out)));
        self.switch_to(next);
        self.field(slot, layout.offsets[0], ty)
    }

    /// The definition and type arguments of a named type; `None` for
    /// primitives.
    fn type_args_of_any(&self, ty: Type) -> (Option<Def>, Vec<Type>) {
        match ty {
            Type::Named(_) => {
                let (def, args) = self.type_args_of(ty);
                (Some(def), args)
            }
            _ => (None, Vec::new()),
        }
    }
}
