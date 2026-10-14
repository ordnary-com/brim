//! Reference interpreter for Brim IR.
//!
//! **This is a temporary execution path**, not Brim's intended backend. It
//! exists so the language can be tested end to end while the native (LLVM)
//! backend is being built, and it will remain useful afterwards as a
//! reference for differential testing. It executes validated IR directly and
//! implements runtime operations through `brim-runtime`, so its behaviour
//! defines what the native backend must reproduce.
//!
//! The interpreter is iterative: Brim calls push frames on a heap-allocated
//! stack rather than recursing in Rust, so deeply recursive Brim programs hit
//! the configurable [`Limits::max_call_depth`] and report a clean run-time
//! error instead of crashing the host process.

use std::io::Write;

use brim_diagnostics::Span;
use brim_ir::{
    BinaryOp, BlockId, Const, FuncId, InstKind, Module, RuntimeFn, Terminator, UnaryOp, ValueId,
};
use brim_runtime::{BrimStr, RuntimeError, ops};

/// Resource limits for a run.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum number of simultaneously active calls, including `main`.
    pub max_call_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_call_depth: 10_000,
        }
    }
}

/// A run-time failure, with the location of the operation that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trap {
    pub error: RuntimeError,
    /// Source location of the failing operation, if known.
    pub span: Option<Span>,
    /// Function names from `main` to the failing function.
    pub backtrace: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    I64(i64),
    Bool(bool),
    Str(BrimStr),
}

impl Value {
    fn i64(&self) -> i64 {
        match self {
            Value::I64(v) => *v,
            other => unreachable!("validated IR: expected i64, found {other:?}"),
        }
    }

    fn bool(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            other => unreachable!("validated IR: expected bool, found {other:?}"),
        }
    }

    fn str(&self) -> &str {
        match self {
            Value::Str(v) => v,
            other => unreachable!("validated IR: expected str, found {other:?}"),
        }
    }
}

struct Frame {
    func: FuncId,
    block: BlockId,
    /// Index of the next instruction in `block`.
    ip: usize,
    slots: Vec<Option<Value>>,
    values: Vec<Option<Value>>,
    /// Where the caller wants the return value.
    return_to: Option<ValueId>,
}

impl Frame {
    fn new(module: &Module, func: FuncId, args: Vec<Value>, return_to: Option<ValueId>) -> Frame {
        let f = module.function(func);
        let mut slots: Vec<Option<Value>> = args.into_iter().map(Some).collect();
        slots.resize(f.slots.len(), None);
        Frame {
            func,
            block: BlockId(0),
            ip: 0,
            slots,
            values: vec![None; f.values.len()],
            return_to,
        }
    }

    fn get(&self, v: ValueId) -> &Value {
        self.values[v.index()]
            .as_ref()
            .expect("validated IR: values are defined before use")
    }
}

/// Runs `module` from its entry point, writing program output to `out`.
///
/// The module must have passed [`brim_ir::validate`]; executing invalid IR
/// may panic.
pub fn run(module: &Module, out: &mut dyn Write, limits: Limits) -> Result<(), Trap> {
    let mut stack = vec![Frame::new(module, module.entry, Vec::new(), None)];
    let result = execute(module, out, limits, &mut stack);
    // Output written before a failure is still delivered.
    let flushed = out.flush();
    match result {
        Err((error, span)) => Err(Trap {
            error,
            span,
            backtrace: stack
                .iter()
                .map(|f| module.function(f.func).name.clone())
                .collect(),
        }),
        Ok(()) => flushed.map_err(|e| Trap {
            error: RuntimeError::Io {
                message: e.to_string(),
            },
            span: None,
            backtrace: Vec::new(),
        }),
    }
}

type Failure = (RuntimeError, Option<Span>);

fn execute(
    module: &Module,
    out: &mut dyn Write,
    limits: Limits,
    stack: &mut Vec<Frame>,
) -> Result<(), Failure> {
    loop {
        let frame = stack.last_mut().expect("the stack is never empty here");
        let func = module.function(frame.func);
        let block = &func.blocks[frame.block.index()];

        let Some(inst) = block.insts.get(frame.ip) else {
            match &block.term {
                Terminator::Return(value) => {
                    let value = value.map(|v| frame.get(v).clone());
                    let finished = stack.pop().expect("frame exists");
                    let Some(caller) = stack.last_mut() else {
                        return Ok(());
                    };
                    if let (Some(dest), Some(value)) = (finished.return_to, value) {
                        caller.values[dest.index()] = Some(value);
                    }
                }
                Terminator::Jump(target) => {
                    frame.block = *target;
                    frame.ip = 0;
                }
                Terminator::Branch {
                    cond,
                    then_block,
                    else_block,
                } => {
                    frame.block = if frame.get(*cond).bool() {
                        *then_block
                    } else {
                        *else_block
                    };
                    frame.ip = 0;
                }
                Terminator::Unreachable => {
                    unreachable!("validated IR: reached an unreachable terminator")
                }
            }
            continue;
        };
        frame.ip += 1;
        let fail = |error: RuntimeError| (error, inst.span);

        let value = match &inst.kind {
            InstKind::Const(Const::I64(v)) => Value::I64(*v),
            InstKind::Const(Const::Bool(v)) => Value::Bool(*v),
            InstKind::Const(Const::Str(v)) => Value::Str(BrimStr::from(v.as_str())),
            InstKind::Load(slot) => frame.slots[slot.index()]
                .clone()
                .expect("slots are initialised before they are read"),
            InstKind::Store(slot, v) => {
                frame.slots[slot.index()] = Some(frame.get(*v).clone());
                continue;
            }
            InstKind::Unary(op, v) => {
                let v = frame.get(*v);
                match op {
                    UnaryOp::INeg => Value::I64(ops::neg(v.i64()).map_err(fail)?),
                    UnaryOp::BNot => Value::Bool(!v.bool()),
                }
            }
            InstKind::Binary(op, a, b) => {
                binary(*op, frame.get(*a), frame.get(*b)).map_err(fail)?
            }
            InstKind::CallRuntime(rt, args) => {
                let args: Vec<&Value> = args.iter().map(|a| frame.get(*a)).collect();
                match call_runtime(*rt, &args, out).map_err(fail)? {
                    Some(v) => v,
                    None => continue,
                }
            }
            InstKind::Call(callee, args) => {
                let args: Vec<Value> = args.iter().map(|a| frame.get(*a).clone()).collect();
                if stack.len() >= limits.max_call_depth {
                    return Err(fail(RuntimeError::StackOverflow {
                        limit: limits.max_call_depth,
                    }));
                }
                let new_frame = Frame::new(module, *callee, args, inst.result);
                stack.push(new_frame);
                continue;
            }
        };
        if let Some(dest) = inst.result {
            frame.values[dest.index()] = Some(value);
        }
    }
}

fn binary(op: BinaryOp, a: &Value, b: &Value) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    Ok(match op {
        IAdd => Value::I64(ops::add(a.i64(), b.i64())?),
        ISub => Value::I64(ops::sub(a.i64(), b.i64())?),
        IMul => Value::I64(ops::mul(a.i64(), b.i64())?),
        IDiv => Value::I64(ops::div(a.i64(), b.i64())?),
        IRem => Value::I64(ops::rem(a.i64(), b.i64())?),
        IEq => Value::Bool(a.i64() == b.i64()),
        INe => Value::Bool(a.i64() != b.i64()),
        ILt => Value::Bool(a.i64() < b.i64()),
        ILe => Value::Bool(a.i64() <= b.i64()),
        IGt => Value::Bool(a.i64() > b.i64()),
        IGe => Value::Bool(a.i64() >= b.i64()),
        BEq => Value::Bool(a.bool() == b.bool()),
        BNe => Value::Bool(a.bool() != b.bool()),
    })
}

fn call_runtime(
    rt: RuntimeFn,
    args: &[&Value],
    out: &mut dyn Write,
) -> Result<Option<Value>, RuntimeError> {
    let io = |e: std::io::Error| RuntimeError::Io {
        message: e.to_string(),
    };
    Ok(match rt {
        RuntimeFn::PrintI64 => {
            brim_runtime::print_i64(out, args[0].i64()).map_err(io)?;
            None
        }
        RuntimeFn::PrintBool => {
            brim_runtime::print_bool(out, args[0].bool()).map_err(io)?;
            None
        }
        RuntimeFn::PrintStr => {
            brim_runtime::print_str(out, args[0].str()).map_err(io)?;
            None
        }
        RuntimeFn::StrConcat => Some(Value::Str(brim_runtime::str_concat(
            args[0].str(),
            args[1].str(),
        ))),
        RuntimeFn::I64ToStr => Some(Value::Str(brim_runtime::i64_to_str(args[0].i64()))),
        RuntimeFn::BoolToStr => Some(Value::Str(brim_runtime::bool_to_str(args[0].bool()))),
        RuntimeFn::StrEq => Some(Value::Bool(brim_runtime::str_eq(
            args[0].str(),
            args[1].str(),
        ))),
    })
}

#[cfg(test)]
mod tests;
