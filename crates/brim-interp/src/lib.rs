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
use std::rc::Rc;

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
            max_call_depth: brim_runtime::MAX_CALL_DEPTH,
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
    /// Field values in field order. Structs are immutable values: `insert`
    /// builds a new one, so sharing the fields is safe.
    Struct(Rc<[Value]>),
    /// Copy-on-write: changing a list that is not shared changes it in place.
    List(Rc<Vec<Value>>),
    /// Copy-on-write, like lists.
    Map(Rc<MapData>),
}

/// A map: entries in insertion order, and where each key is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct MapData {
    entries: Vec<(Value, Value)>,
    index: std::collections::HashMap<MapKey, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MapKey {
    I64(i64),
    Bool(bool),
    Str(BrimStr),
}

impl MapKey {
    fn of(value: &Value) -> MapKey {
        match value {
            Value::I64(v) => MapKey::I64(*v),
            Value::Bool(v) => MapKey::Bool(*v),
            Value::Str(v) => MapKey::Str(v.clone()),
            other => unreachable!("validated IR: map keys are i64, bool or str, found {other:?}"),
        }
    }
}

impl MapData {
    fn find(&self, key: &Value) -> Option<usize> {
        self.index.get(&MapKey::of(key)).copied()
    }

    fn insert(&mut self, key: Value, value: Value) {
        match self.find(&key) {
            Some(i) => self.entries[i].1 = value,
            None => {
                self.index.insert(MapKey::of(&key), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    fn remove_at(&mut self, i: usize) {
        self.entries.remove(i);
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (MapKey::of(k), i))
            .collect();
    }
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

    fn list(&self) -> &Rc<Vec<Value>> {
        match self {
            Value::List(v) => v,
            other => unreachable!("validated IR: expected a list, found {other:?}"),
        }
    }

    fn map(&self) -> &Rc<MapData> {
        match self {
            Value::Map(v) => v,
            other => unreachable!("validated IR: expected a map, found {other:?}"),
        }
    }

    fn into_map(self) -> Rc<MapData> {
        match self {
            Value::Map(v) => v,
            other => unreachable!("validated IR: expected a map, found {other:?}"),
        }
    }

    fn into_list(self) -> Rc<Vec<Value>> {
        match self {
            Value::List(v) => v,
            other => unreachable!("validated IR: expected a list, found {other:?}"),
        }
    }

    fn into_fields(self) -> Rc<[Value]> {
        match self {
            Value::Struct(v) => v,
            other => unreachable!("validated IR: expected a struct, found {other:?}"),
        }
    }

    fn fields(&self) -> &Rc<[Value]> {
        match self {
            Value::Struct(v) => v,
            other => unreachable!("validated IR: expected a struct, found {other:?}"),
        }
    }

    fn str(&self) -> &str {
        match self {
            Value::Str(v) => v,
            other => unreachable!("validated IR: expected str, found {other:?}"),
        }
    }
}

/// For each instruction of a function, the values it uses for the last
/// time in its block: `[block][instruction]`. The interpreter drops them
/// right after the instruction, so a list or struct that is changed in place
/// is not kept alive (and copied) because of an old temporary.
type LastUses = Vec<Vec<Vec<ValueId>>>;

fn last_uses(func: &brim_ir::Function) -> LastUses {
    func.blocks
        .iter()
        .map(|block| {
            let mut later: std::collections::HashSet<ValueId> = match &block.term {
                Terminator::Return(Some(v)) => [*v].into(),
                Terminator::Branch { cond, .. } => [*cond].into(),
                _ => Default::default(),
            };
            let mut dead: Vec<Vec<ValueId>> = vec![Vec::new(); block.insts.len()];
            for (i, inst) in block.insts.iter().enumerate().rev() {
                for operand in inst.kind.operands() {
                    if later.insert(operand) {
                        dead[i].push(operand);
                    }
                }
            }
            dead
        })
        .collect()
}

struct Frame {
    func: FuncId,
    last_uses: Rc<LastUses>,
    /// The instruction executed last, whose dead operands are still to be
    /// dropped.
    pending: Option<(BlockId, usize)>,
    block: BlockId,
    /// Index of the next instruction in `block`.
    ip: usize,
    slots: Vec<Option<Value>>,
    values: Vec<Option<Value>>,
    /// Where the caller wants the return value.
    return_to: Option<ValueId>,
}

impl Frame {
    fn new(
        module: &Module,
        func: FuncId,
        args: Vec<Value>,
        return_to: Option<ValueId>,
        cache: &mut [Option<Rc<LastUses>>],
    ) -> Frame {
        let f = module.function(func);
        let last_uses = cache[func.index()]
            .get_or_insert_with(|| Rc::new(last_uses(f)))
            .clone();
        let mut slots: Vec<Option<Value>> = args.into_iter().map(Some).collect();
        slots.resize(f.slots.len(), None);
        Frame {
            func,
            last_uses,
            pending: None,
            block: BlockId(0),
            ip: 0,
            slots,
            values: vec![None; f.values.len()],
            return_to,
        }
    }

    /// Takes a consumed operand out of the frame, so that a list or struct
    /// that nothing else refers to can be changed in place.
    fn take(&mut self, v: ValueId) -> Value {
        self.values[v.index()]
            .take()
            .expect("validated IR: values are defined before use and consumed once")
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
    let mut cache = vec![None; module.functions.len()];
    let mut stack = vec![Frame::new(
        module,
        module.entry,
        Vec::new(),
        None,
        &mut cache,
    )];
    let result = execute(module, out, limits, &mut stack, &mut cache);
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
    cache: &mut [Option<Rc<LastUses>>],
) -> Result<(), Failure> {
    loop {
        let frame = stack.last_mut().expect("the stack is never empty here");
        if let Some((block, ip)) = frame.pending.take() {
            let last_uses = Rc::clone(&frame.last_uses);
            for v in &last_uses[block.index()][ip] {
                frame.values[v.index()] = None;
            }
        }
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
        frame.pending = Some((frame.block, frame.ip));
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
            InstKind::MakeStruct(_, args) => {
                Value::Struct(args.iter().map(|a| frame.get(*a).clone()).collect())
            }
            InstKind::Extract(v, index) => frame.get(*v).fields()[*index].clone(),
            InstKind::Insert(v, index, x) => {
                let x = frame.get(*x).clone();
                let mut fields = frame.take(*v).into_fields();
                Rc::make_mut(&mut fields)[*index] = x;
                Value::Struct(fields)
            }
            InstKind::Take(slot) => frame.slots[slot.index()]
                .take()
                .expect("slots are initialised before they are read"),
            InstKind::ListNew(_, args) => Value::List(Rc::new(
                args.iter().map(|a| frame.get(*a).clone()).collect(),
            )),
            InstKind::ListLen(v) => Value::I64(frame.get(*v).list().len() as i64),
            InstKind::ListGet(v, i) => {
                let list = frame.get(*v).list();
                let index = checked_index(frame.get(*i).i64(), list.len(), false).map_err(fail)?;
                list[index].clone()
            }
            InstKind::ListSet(v, i, x) => {
                let (i, x) = (frame.get(*i).i64(), frame.get(*x).clone());
                let mut list = frame.take(*v).into_list();
                let index = checked_index(i, list.len(), false).map_err(fail)?;
                Rc::make_mut(&mut list)[index] = x;
                Value::List(list)
            }
            InstKind::ListPush(v, x) => {
                let x = frame.get(*x).clone();
                let mut list = frame.take(*v).into_list();
                Rc::make_mut(&mut list).push(x);
                Value::List(list)
            }
            InstKind::ListTruncate(v, n) => {
                let n = frame.get(*n).i64();
                let mut list = frame.take(*v).into_list();
                let n = checked_index(n, list.len(), true).map_err(fail)?;
                Rc::make_mut(&mut list).truncate(n);
                Value::List(list)
            }
            InstKind::ListInsert(v, i, x) => {
                let (i, x) = (frame.get(*i).i64(), frame.get(*x).clone());
                let mut list = frame.take(*v).into_list();
                let index = checked_index(i, list.len(), true).map_err(fail)?;
                Rc::make_mut(&mut list).insert(index, x);
                Value::List(list)
            }
            InstKind::ListRemove(v, i) => {
                let i = frame.get(*i).i64();
                let mut list = frame.take(*v).into_list();
                let index = checked_index(i, list.len(), false).map_err(fail)?;
                Rc::make_mut(&mut list).remove(index);
                Value::List(list)
            }
            InstKind::MapNew(_) => Value::Map(Rc::default()),
            InstKind::MapLen(v) => Value::I64(frame.get(*v).map().entries.len() as i64),
            InstKind::MapFind(v, k) => {
                let found = frame.get(*v).map().find(frame.get(*k));
                Value::I64(found.map_or(-1, |i| i as i64))
            }
            InstKind::MapIndex(v, k) => match frame.get(*v).map().find(frame.get(*k)) {
                Some(i) => Value::I64(i as i64),
                None => return Err(fail(RuntimeError::KeyNotFound)),
            },
            InstKind::MapKeyAt(v, i) => {
                let i = frame.get(*i).i64() as usize;
                frame.get(*v).map().entries[i].0.clone()
            }
            InstKind::MapValueAt(v, i) => {
                let i = frame.get(*i).i64() as usize;
                frame.get(*v).map().entries[i].1.clone()
            }
            InstKind::MapInsert(v, k, x) => {
                let (k, x) = (frame.get(*k).clone(), frame.get(*x).clone());
                let mut map = frame.take(*v).into_map();
                Rc::make_mut(&mut map).insert(k, x);
                Value::Map(map)
            }
            InstKind::MapSetValueAt(v, i, x) => {
                let (i, x) = (frame.get(*i).i64() as usize, frame.get(*x).clone());
                let mut map = frame.take(*v).into_map();
                Rc::make_mut(&mut map).entries[i].1 = x;
                Value::Map(map)
            }
            InstKind::MapRemoveAt(v, i) => {
                let i = frame.get(*i).i64() as usize;
                let mut map = frame.take(*v).into_map();
                Rc::make_mut(&mut map).remove_at(i);
                Value::Map(map)
            }
            InstKind::ListContains(v, x) => {
                let x = frame.get(*x);
                Value::Bool(frame.get(*v).list().iter().any(|e| e == x))
            }
            InstKind::ListJoin(v, sep) => {
                let sep = frame.get(*sep).str();
                let parts: Vec<&str> = frame.get(*v).list().iter().map(Value::str).collect();
                Value::Str(BrimStr::from(parts.join(sep)))
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
                let new_frame = Frame::new(module, *callee, args, inst.result, cache);
                stack.push(new_frame);
                continue;
            }
        };
        if let Some(dest) = inst.result {
            frame.values[dest.index()] = Some(value);
        }
    }
}

/// `index` as a position in a list of `len` elements; `len` itself is
/// allowed when `inclusive` (inserting at the end, truncating to the length).
fn checked_index(index: i64, len: usize, inclusive: bool) -> Result<usize, RuntimeError> {
    let limit = if inclusive { len + 1 } else { len };
    match usize::try_from(index) {
        Ok(i) if i < limit => Ok(i),
        _ => Err(RuntimeError::IndexOutOfBounds {
            index,
            len: len as i64,
        }),
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
