//! Structural and type validation of IR modules.
//!
//! Lowering is expected to always produce valid IR, so a validation failure
//! indicates a compiler bug. Backends run the validator before consuming a
//! module so that such bugs are caught early instead of producing incorrect
//! programs.

use std::fmt;

use crate::{BlockId, Function, InstKind, Module, StructId, Terminator, Type, ValueId};

/// One violation of the IR rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub function: String,
    pub block: Option<BlockId>,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.block {
            Some(block) => write!(f, "in `{}` {block}: {}", self.function, self.message),
            None => write!(f, "in `{}`: {}", self.function, self.message),
        }
    }
}

/// Checks every rule described in the crate documentation.
pub fn validate(module: &Module) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();
    match module.functions.get(module.entry.index()) {
        None => errors.push(ValidationError {
            function: "<module>".into(),
            block: None,
            message: format!("entry {} does not exist", module.entry),
        }),
        Some(entry) => {
            if entry.param_count != 0 || entry.ret != Type::Void {
                errors.push(ValidationError {
                    function: entry.name.clone(),
                    block: None,
                    message: "the entry function must take no parameters and return void".into(),
                });
            }
        }
    }
    validate_structs(module, &mut errors);
    for function in &module.functions {
        FnValidator {
            module,
            function,
            errors: &mut errors,
            current_block: None,
        }
        .run();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Field types must exist and not be void, and no struct may contain itself.
fn validate_structs(module: &Module, errors: &mut Vec<ValidationError>) {
    let mut error = |name: &str, message: String| {
        errors.push(ValidationError {
            function: format!("struct {name}"),
            block: None,
            message,
        });
    };
    for def in &module.structs {
        for field in &def.fields {
            match field.ty {
                Type::Void => error(&def.name, format!("field `{}` has type void", field.name)),
                Type::Struct(id) if id.index() >= module.structs.len() => {
                    error(
                        &def.name,
                        format!("field `{}` has nonexistent type {id}", field.name),
                    );
                }
                _ => {}
            }
        }
    }
    // Contains-graph cycle detection (0 = unvisited, 1 = in progress, 2 = done).
    fn visit(module: &Module, id: usize, state: &mut [u8]) -> bool {
        match state[id] {
            1 => return true,
            2 => return false,
            _ => {}
        }
        state[id] = 1;
        for field in &module.structs[id].fields {
            if let Type::Struct(next) = field.ty
                && next.index() < module.structs.len()
                && visit(module, next.index(), state)
            {
                return true;
            }
        }
        state[id] = 2;
        false
    }
    let mut state = vec![0u8; module.structs.len()];
    for id in 0..module.structs.len() {
        if visit(module, id, &mut state) {
            error(&module.structs[id].name, "struct contains itself".into());
            return;
        }
    }
}

struct FnValidator<'a> {
    module: &'a Module,
    function: &'a Function,
    errors: &'a mut Vec<ValidationError>,
    current_block: Option<BlockId>,
}

impl FnValidator<'_> {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(ValidationError {
            function: self.function.name.clone(),
            block: self.current_block,
            message: message.into(),
        });
    }

    fn run(&mut self) {
        let f = self.function;
        if f.param_count > f.slots.len() {
            self.error(format!(
                "{} parameters but only {} slots",
                f.param_count,
                f.slots.len()
            ));
            return;
        }
        if let Some(i) = f.slots.iter().position(|s| s.ty == Type::Void) {
            self.error(format!("slot s{i} has type void"));
        }
        let types = f
            .slots
            .iter()
            .map(|s| s.ty)
            .chain(f.values.iter().copied())
            .chain([f.ret]);
        for ty in types {
            if let Type::Struct(id) = ty
                && id.index() >= self.module.structs.len()
            {
                self.error(format!("nonexistent struct type {id}"));
                return;
            }
        }
        if f.blocks.is_empty() {
            self.error("function has no blocks");
            return;
        }

        let mut defined_anywhere = vec![false; f.values.len()];
        for (index, block) in f.blocks.iter().enumerate() {
            self.current_block = Some(BlockId(index as u32));
            // Values defined so far in this block.
            let mut local = vec![false; f.values.len()];
            for inst in &block.insts {
                self.check_inst(&inst.kind, inst.result, &local);
                if let Some(result) = inst.result {
                    match defined_anywhere.get_mut(result.index()) {
                        None => self.error(format!("{result} is out of range")),
                        Some(true) => self.error(format!("{result} is defined more than once")),
                        Some(seen) => {
                            *seen = true;
                            local[result.index()] = true;
                        }
                    }
                }
            }
            self.check_terminator(&block.term, &local);
        }
    }

    /// Type of `value` if it is defined earlier in the current block.
    fn operand(&mut self, value: ValueId, local: &[bool]) -> Option<Type> {
        match local.get(value.index()) {
            Some(true) => Some(self.function.values[value.index()]),
            Some(false) => {
                self.error(format!(
                    "{value} is used before being defined in this block"
                ));
                None
            }
            None => {
                self.error(format!("{value} is out of range"));
                None
            }
        }
    }

    fn expect_operand(&mut self, value: ValueId, expected: Type, local: &[bool]) {
        if let Some(ty) = self.operand(value, local)
            && ty != expected
        {
            self.error(format!("{value} has type {ty}, expected {expected}"));
        }
    }

    /// For `extract`/`insert`: the struct type of `v` and the type of its
    /// field `index`.
    fn struct_field(
        &mut self,
        v: ValueId,
        index: usize,
        local: &[bool],
    ) -> Option<(StructId, Type)> {
        let ty = self.operand(v, local)?;
        let Type::Struct(id) = ty else {
            self.error(format!("{v} has type {ty}, expected a struct"));
            return None;
        };
        match self.module.structs[id.index()].fields.get(index) {
            Some(field) => Some((id, field.ty)),
            None => {
                self.error(format!("{id} has no field {index}"));
                None
            }
        }
    }

    fn check_args(&mut self, what: &str, args: &[ValueId], params: &[Type], local: &[bool]) {
        if args.len() != params.len() {
            self.error(format!(
                "{what} expects {} arguments, got {}",
                params.len(),
                args.len()
            ));
            return;
        }
        for (arg, param) in args.iter().zip(params) {
            self.expect_operand(*arg, *param, local);
        }
    }

    /// Checks an instruction's operands and the type of its result.
    fn check_inst(&mut self, kind: &InstKind, result: Option<ValueId>, local: &[bool]) {
        let f = self.function;
        let produced: Option<Type> = match kind {
            InstKind::Const(c) => Some(c.ty()),
            InstKind::Load(slot) => match f.slots.get(slot.index()) {
                Some(s) => Some(s.ty),
                None => {
                    self.error(format!("{slot} does not exist"));
                    return;
                }
            },
            InstKind::Store(slot, value) => {
                match f.slots.get(slot.index()) {
                    Some(s) => self.expect_operand(*value, s.ty, local),
                    None => self.error(format!("{slot} does not exist")),
                }
                None
            }
            InstKind::Unary(op, v) => {
                let (operand, out) = op.signature();
                self.expect_operand(*v, operand, local);
                Some(out)
            }
            InstKind::Binary(op, a, b) => {
                let (operand, out) = op.signature();
                self.expect_operand(*a, operand, local);
                self.expect_operand(*b, operand, local);
                Some(out)
            }
            InstKind::Call(func, args) => {
                let Some(callee) = self.module.functions.get(func.index()) else {
                    self.error(format!("call to nonexistent {func}"));
                    return;
                };
                let params: Vec<Type> = callee.param_types().collect();
                self.check_args(&format!("`{}`", callee.name), args, &params, local);
                (callee.ret != Type::Void).then_some(callee.ret)
            }
            InstKind::CallRuntime(rt, args) => {
                let params: Vec<Type> = rt.params().iter().map(|t| Type::from(*t)).collect();
                self.check_args(&format!("@{rt}"), args, &params, local);
                let ret = Type::from(rt.ret());
                (ret != Type::Void).then_some(ret)
            }
            InstKind::MakeStruct(id, args) => {
                let Some(def) = self.module.structs.get(id.index()) else {
                    self.error(format!("make of nonexistent struct {id}"));
                    return;
                };
                let fields: Vec<Type> = def.fields.iter().map(|f| f.ty).collect();
                self.check_args(&format!("make {id}"), args, &fields, local);
                Some(Type::Struct(*id))
            }
            InstKind::Extract(v, index) => {
                let Some((_, ty)) = self.struct_field(*v, *index, local) else {
                    return;
                };
                Some(ty)
            }
            InstKind::Insert(v, index, x) => {
                let Some((id, ty)) = self.struct_field(*v, *index, local) else {
                    return;
                };
                self.expect_operand(*x, ty, local);
                Some(Type::Struct(id))
            }
        };
        match (produced, result) {
            (None, None) => {}
            (Some(_), None) if matches!(kind, InstKind::Call(..) | InstKind::CallRuntime(..)) => {
                // Discarding a call's result is allowed.
            }
            (Some(ty), None) => self.error(format!("instruction producing {ty} has no result")),
            (None, Some(v)) => self.error(format!(
                "{v} is assigned from an instruction without a result"
            )),
            (Some(ty), Some(v)) => match f.values.get(v.index()) {
                Some(declared) if *declared != ty => {
                    self.error(format!("{v} is declared as {declared} but assigned {ty}"));
                }
                _ => {}
            },
        }
    }

    fn check_terminator(&mut self, term: &Terminator, local: &[bool]) {
        let f = self.function;
        let block_exists = |b: BlockId| b.index() < f.blocks.len();
        match term {
            Terminator::Return(None) if f.ret != Type::Void => {
                self.error(format!("missing return value of type {}", f.ret));
            }
            Terminator::Return(None) => {}
            Terminator::Return(Some(v)) => {
                if f.ret == Type::Void {
                    self.error("void function returns a value");
                } else {
                    self.expect_operand(*v, f.ret, local);
                }
            }
            Terminator::Jump(b) => {
                if !block_exists(*b) {
                    self.error(format!("jump to nonexistent {b}"));
                }
            }
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                self.expect_operand(*cond, Type::Bool, local);
                for b in [then_block, else_block] {
                    if !block_exists(*b) {
                        self.error(format!("branch to nonexistent {b}"));
                    }
                }
            }
            Terminator::Unreachable => {}
        }
    }
}
