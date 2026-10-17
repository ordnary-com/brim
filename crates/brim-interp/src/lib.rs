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

#[cfg(not(target_family = "wasm"))]
mod https;
#[cfg(target_family = "wasm")]
mod https {
    /// WebAssembly has no libcurl: HTTPS requests fail.
    pub(crate) fn request(
        _method: &str,
        _url: &str,
        _headers: &str,
        _body: &str,
        _timeout: i64,
    ) -> Result<String, String> {
        Err("HTTPS is not available here".into())
    }
}

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

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
    /// Maximum number of instructions and terminators to execute, for
    /// embedders that run untrusted programs; `None` (the default) for no
    /// limit. Exceeding it is the run-time error `R0009`.
    pub max_steps: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_call_depth: brim_runtime::MAX_CALL_DEPTH,
            max_steps: None,
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
    /// The bits of an `f64` (so `Value` stays `Eq`).
    F64(u64),
    Bool(bool),
    Str(BrimStr),
    /// Field values in field order. Structs are immutable values: `insert`
    /// builds a new one, so sharing the fields is safe.
    Struct(Arc<[Value]>),
    /// Copy-on-write: changing a list that is not shared changes it in place.
    List(Arc<Vec<Value>>),
    /// Copy-on-write, like lists.
    Map(Arc<MapData>),
    /// A function and its environment (a list).
    Closure(FuncId, Box<Value>),
}

/// Values nest without limit (a long chain of boxes is a list in a struct in
/// a list ...), so dropping one recursively could overflow the stack. The
/// values inside a value that is about to be freed are moved out and
/// dropped one at a time instead.
impl Drop for Value {
    #[inline]
    fn drop(&mut self) {
        // Most values hold nothing that needs care: numbers, strings, and
        // lists, structs and maps that are still shared.
        let alone = match self {
            Value::I64(_) | Value::F64(_) | Value::Bool(_) | Value::Str(_) => false,
            Value::Struct(fields) => Arc::strong_count(fields) == 1,
            Value::List(items) => Arc::strong_count(items) == 1,
            Value::Map(map) => Arc::strong_count(map) == 1,
            Value::Closure(..) => true,
        };
        if alone {
            drop_nested(self);
        }
    }
}

/// Drops the values inside `value` one at a time.
#[inline(never)]
fn drop_nested(value: &mut Value) {
    {
        let mut pending = Vec::new();
        take_contents(value, &mut pending);
        while let Some(mut value) = pending.pop() {
            take_contents(&mut value, &mut pending);
        }
    }
}

/// Moves the values inside `value` to `out`, if nothing else shares them.
fn take_contents(value: &mut Value, out: &mut Vec<Value>) {
    match value {
        Value::List(items) => {
            if let Some(items) = Arc::get_mut(items) {
                out.append(items);
            }
        }
        Value::Struct(fields) => {
            if let Some(fields) = Arc::get_mut(fields) {
                out.extend(
                    fields
                        .iter_mut()
                        .map(|f| std::mem::replace(f, Value::Bool(false))),
                );
            }
        }
        Value::Map(map) => {
            if let Some(map) = Arc::get_mut(map) {
                for (key, value) in map.entries.drain(..) {
                    out.push(key);
                    out.push(value);
                }
            }
        }
        Value::Closure(_, env) => out.push(std::mem::replace(env, Value::Bool(false))),
        Value::I64(_) | Value::F64(_) | Value::Bool(_) | Value::Str(_) => {}
    }
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
    fn f64(&self) -> f64 {
        match self {
            Value::F64(bits) => f64::from_bits(*bits),
            other => unreachable!("validated IR: expected f64, found {other:?}"),
        }
    }

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

    fn list(&self) -> &Arc<Vec<Value>> {
        match self {
            Value::List(v) => v,
            other => unreachable!("validated IR: expected a list, found {other:?}"),
        }
    }

    fn map(&self) -> &Arc<MapData> {
        match self {
            Value::Map(v) => v,
            other => unreachable!("validated IR: expected a map, found {other:?}"),
        }
    }

    // `Value` implements `Drop`, so these take the contents out instead of
    // moving them.
    fn into_map(mut self) -> Arc<MapData> {
        match &mut self {
            Value::Map(v) => std::mem::take(v),
            other => unreachable!("validated IR: expected a map, found {other:?}"),
        }
    }

    fn into_list(mut self) -> Arc<Vec<Value>> {
        match &mut self {
            Value::List(v) => std::mem::take(v),
            other => unreachable!("validated IR: expected a list, found {other:?}"),
        }
    }

    fn into_fields(mut self) -> Arc<[Value]> {
        match &mut self {
            Value::Struct(v) => std::mem::take(v),
            other => unreachable!("validated IR: expected a struct, found {other:?}"),
        }
    }

    fn fields(&self) -> &Arc<[Value]> {
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

/// What a running program can use from its environment.
pub struct Host<'a> {
    /// Standard output (`print`).
    pub out: &'a mut (dyn Write + Send),
    /// Standard error (`eprint`).
    pub err: &'a mut (dyn Write + Send),
    /// Standard input (`read_line`).
    pub input: &'a mut (dyn BufRead + Send),
    /// The program's arguments (`args()`), without a program name.
    pub args: Vec<String>,
    /// The directory relative file paths are resolved against.
    pub cwd: std::path::PathBuf,
}

impl std::fmt::Debug for Host<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

/// Runs `module` from its entry point. Returns the program's exit code: 0,
/// or the code it passed to `exit`.
///
/// The module must have passed [`brim_ir::validate`]; executing invalid IR
/// may panic.
pub fn run(module: &Module, host: &mut Host<'_>, limits: Limits) -> Result<i32, Trap> {
    run_function(module, module.entry, host, limits)
}

/// Runs one function of `module`, which must take no parameters and return
/// `void`, as if it were the entry point. `brim test` runs each test so.
///
/// Tasks the program starts (`std.task`) run on threads of their own; the
/// run ends when the function returns, stopping the tasks still running.
pub fn run_function(
    module: &Module,
    function: brim_ir::FuncId,
    host: &mut Host<'_>,
    limits: Limits,
) -> Result<i32, Trap> {
    let shared = Shared {
        out: Mutex::new(&mut *host.out),
        err: Mutex::new(&mut *host.err),
        input: Mutex::new(&mut *host.input),
        args: host.args.clone(),
        cwd: host.cwd.clone(),
        sockets: brim_runtime::net::Sockets::default(),
        tasks: Mutex::new(Tasks::default()),
        wake: Condvar::new(),
        stopping: AtomicBool::new(false),
        ended: Mutex::new(None),
    };
    let (result, backtrace) = std::thread::scope(|scope| {
        let mut cache = vec![None; module.functions.len()];
        let mut stack = vec![Frame::new(module, function, Vec::new(), None, &mut cache)];
        let mut state = State::new(&shared, scope);
        let result = execute(module, &mut state, limits, &mut stack, &mut cache);
        let backtrace: Vec<String> = stack
            .iter()
            .map(|f| module.function(f.func).name.clone())
            .collect();
        shared.stop();
        (result, backtrace)
    });
    // A task that ended the program decides how it ended.
    let ended = lock(&shared.ended).take();
    // Output written before a failure is still delivered.
    let flushed = lock(&shared.out).flush().and(lock(&shared.err).flush());
    let result = match (ended, result) {
        (Some(ended), _) => ended,
        (None, Ok(Outcome::Returned | Outcome::Stopped)) => Ok(0),
        (None, Ok(Outcome::Exit(code))) => Ok(code),
        (None, Err((error, span))) => Err(Trap {
            error,
            span,
            backtrace,
        }),
    };
    let code = result?;
    flushed.map(|()| code).map_err(|e| Trap {
        error: RuntimeError::Io {
            message: e.to_string(),
        },
        span: None,
        backtrace: Vec::new(),
    })
}

/// Locks a mutex, also after a thread panicked while holding it.
fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the tasks of a run share.
struct Shared<'h> {
    out: Mutex<&'h mut (dyn Write + Send)>,
    err: Mutex<&'h mut (dyn Write + Send)>,
    input: Mutex<&'h mut (dyn BufRead + Send)>,
    args: Vec<String>,
    cwd: std::path::PathBuf,
    /// Open sockets (`std.net`).
    sockets: brim_runtime::net::Sockets,
    tasks: Mutex<Tasks>,
    /// Wakes the tasks waiting for a task, a channel or the end.
    wake: Condvar,
    /// Set when the program ends: tasks stop at their next instruction.
    stopping: AtomicBool,
    /// How a task other than the first ended the program: `exit` or a
    /// run-time error.
    ended: Mutex<Option<Result<i32, Trap>>>,
}

impl Shared<'_> {
    /// Ends the run: every task stops.
    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.sockets.stop();
        let _tasks = lock(&self.tasks);
        self.wake.notify_all();
    }

    /// A task other than the first ended the program.
    fn end(&self, how: Result<i32, Trap>) {
        lock(&self.ended).get_or_insert(how);
        self.stop();
    }

    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Relaxed)
    }
}

/// Tasks and channels, by handle. Handles are never reused.
#[derive(Default)]
struct Tasks {
    next: i64,
    /// The tasks that can be joined: `None` while running, then the list
    /// their function returned.
    results: HashMap<i64, Option<Value>>,
    channels: HashMap<i64, Channel>,
}

/// A channel; it is removed once closed and drained, and a handle that is
/// not here is a closed channel.
#[derive(Default)]
struct Channel {
    queue: VecDeque<Value>,
    /// The most values queued before `send` waits; 0 for no limit.
    capacity: usize,
    closed: bool,
}

/// How a task's instructions ended.
enum Outcome {
    /// Its function returned.
    Returned,
    /// It called `exit`.
    Exit(i32),
    /// The run is ending.
    Stopped,
}

/// One task's state, and what it shares with the others.
struct State<'s, 'e, 'h> {
    shared: &'e Shared<'h>,
    scope: &'s std::thread::Scope<'s, 'e>,
    /// The error of the last I/O function (`LastError`); empty on success.
    last_error: String,
    /// The output of the last `ProcessRun`.
    process: brim_runtime::ProcessOutput,
    /// What the task's function returned.
    returned: Option<Value>,
}

impl<'s, 'e, 'h> State<'s, 'e, 'h> {
    fn new(shared: &'e Shared<'h>, scope: &'s std::thread::Scope<'s, 'e>) -> Self {
        State {
            shared,
            scope,
            last_error: String::new(),
            process: brim_runtime::ProcessOutput::default(),
            returned: None,
        }
    }

    fn out(&self) -> MutexGuard<'e, &'h mut (dyn Write + Send)> {
        lock(&self.shared.out)
    }

    fn err(&self) -> MutexGuard<'e, &'h mut (dyn Write + Send)> {
        lock(&self.shared.err)
    }

    /// Starts a task running `closure`; its handle.
    fn start_task(
        &mut self,
        module: &'e Module,
        limits: Limits,
        closure: Value,
        joinable: bool,
    ) -> Result<i64, RuntimeError> {
        let shared = self.shared;
        let scope = self.scope;
        shared.sockets.share();
        let id = {
            let mut tasks = lock(&shared.tasks);
            tasks.next += 1;
            if joinable {
                let id = tasks.next;
                tasks.results.insert(id, None);
            }
            tasks.next
        };
        let joined = joinable.then_some(id);
        std::thread::Builder::new()
            .spawn_scoped(scope, move || {
                run_task(module, shared, scope, limits, closure, joined)
            })
            .map_err(|_| RuntimeError::NoTasks)?;
        Ok(id)
    }

    /// Waits for a task and takes the list its function returned.
    fn join(&mut self, id: i64) -> Result<Option<Value>, RuntimeError> {
        let shared = self.shared;
        let mut tasks = lock(&shared.tasks);
        loop {
            if shared.stopping() {
                return Ok(None);
            }
            match tasks.results.get(&id) {
                None => return Err(RuntimeError::TaskJoined),
                Some(Some(_)) => return Ok(tasks.results.remove(&id).flatten()),
                Some(None) => {
                    tasks = shared.wake.wait(tasks).unwrap_or_else(|e| e.into_inner());
                }
            }
        }
    }

    fn new_channel(&mut self, capacity: i64) -> i64 {
        let mut tasks = lock(&self.shared.tasks);
        tasks.next += 1;
        let id = tasks.next;
        tasks.channels.insert(
            id,
            Channel {
                capacity: usize::try_from(capacity).unwrap_or(0),
                ..Channel::default()
            },
        );
        id
    }

    /// Queues `value`; whether the channel was open. `None` when the run
    /// is ending.
    fn send(&mut self, id: i64, value: Value) -> Option<bool> {
        let shared = self.shared;
        let mut tasks = lock(&shared.tasks);
        loop {
            if shared.stopping() {
                return None;
            }
            let Some(channel) = tasks.channels.get_mut(&id) else {
                return Some(false);
            };
            if channel.closed {
                return Some(false);
            }
            if channel.capacity == 0 || channel.queue.len() < channel.capacity {
                channel.queue.push_back(value);
                shared.wake.notify_all();
                return Some(true);
            }
            tasks = shared.wake.wait(tasks).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The next value of a channel, waiting for one: `Some(None)` once it
    /// is closed and drained, `None` when the run is ending.
    fn receive(&mut self, id: i64) -> Option<Option<Value>> {
        let shared = self.shared;
        let mut tasks = lock(&shared.tasks);
        loop {
            if shared.stopping() {
                return None;
            }
            let Some(channel) = tasks.channels.get_mut(&id) else {
                return Some(None);
            };
            if let Some(value) = channel.queue.pop_front() {
                if channel.closed && channel.queue.is_empty() {
                    tasks.channels.remove(&id);
                }
                // A sender may wait for room.
                shared.wake.notify_all();
                return Some(Some(value));
            }
            if channel.closed {
                tasks.channels.remove(&id);
                return Some(None);
            }
            tasks = shared.wake.wait(tasks).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close_channel(&mut self, id: i64) {
        let mut tasks = lock(&self.shared.tasks);
        if let Some(channel) = tasks.channels.get_mut(&id) {
            channel.closed = true;
            if channel.queue.is_empty() {
                tasks.channels.remove(&id);
            }
        }
        self.shared.wake.notify_all();
    }

    /// Waits `millis` milliseconds, or until the run ends.
    fn sleep(&mut self, millis: i64) {
        let duration = std::time::Duration::from_millis(u64::try_from(millis).unwrap_or(0));
        let deadline = std::time::Instant::now() + duration;
        let shared = self.shared;
        let mut tasks = lock(&shared.tasks);
        while !shared.stopping() {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            tasks = shared
                .wake
                .wait_timeout(tasks, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Runs a task's closure to the end, on its own thread.
fn run_task<'s, 'e, 'h>(
    module: &'e Module,
    shared: &'e Shared<'h>,
    scope: &'s std::thread::Scope<'s, 'e>,
    limits: Limits,
    closure: Value,
    joined: Option<i64>,
) {
    let Value::Closure(func, env) = &closure else {
        unreachable!("validated IR: tasks run closures");
    };
    let mut cache = vec![None; module.functions.len()];
    let env = (**env).clone();
    let mut stack = vec![Frame::new(module, *func, vec![env], None, &mut cache)];
    let mut state = State::new(shared, scope);
    match execute(module, &mut state, limits, &mut stack, &mut cache) {
        Ok(Outcome::Returned) => {
            if let Some(id) = joined {
                let mut tasks = lock(&shared.tasks);
                tasks.results.insert(id, state.returned.take());
                shared.wake.notify_all();
            }
        }
        Ok(Outcome::Exit(code)) => shared.end(Ok(code)),
        Ok(Outcome::Stopped) => {}
        Err((error, span)) => shared.end(Err(Trap {
            error,
            span,
            backtrace: stack
                .iter()
                .map(|f| module.function(f.func).name.clone())
                .collect(),
        })),
    }
}

type Failure = (RuntimeError, Option<Span>);

fn execute<'s, 'e, 'h>(
    module: &'e Module,
    state: &mut State<'s, 'e, 'h>,
    limits: Limits,
    stack: &mut Vec<Frame>,
    cache: &mut [Option<Rc<LastUses>>],
) -> Result<Outcome, Failure> {
    let mut steps: u64 = 0;
    loop {
        if state.shared.stopping() {
            return Ok(Outcome::Stopped);
        }
        if let Some(limit) = limits.max_steps {
            steps += 1;
            if steps > limit {
                let frame = stack.last().expect("the stack is never empty here");
                let block = &module.function(frame.func).blocks[frame.block.index()];
                let span = block.insts.get(frame.ip).and_then(|i| i.span);
                return Err((RuntimeError::StepLimit { limit }, span));
            }
        }
        let frame = stack.last_mut().expect("the stack is never empty here");
        if let Some((block, ip)) = frame.pending.take() {
            // Disjoint fields: no need to clone the `Rc`.
            for v in &frame.last_uses[block.index()][ip] {
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
                        state.returned = value;
                        return Ok(Outcome::Returned);
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
            InstKind::Const(Const::F64(bits)) => Value::F64(*bits),
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
                    UnaryOp::FNeg => float(-v.f64()),
                    UnaryOp::BNot => Value::Bool(!v.bool()),
                    UnaryOp::Fit { min, max, op } => {
                        Value::I64(ops::fit(v.i64(), *min, *max, op).map_err(fail)?)
                    }
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
                Arc::make_mut(&mut fields)[*index] = x;
                Value::Struct(fields)
            }
            InstKind::Take(slot) => frame.slots[slot.index()]
                .take()
                .expect("slots are initialised before they are read"),
            InstKind::ListNew(_, args) => Value::List(Arc::new(
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
                Arc::make_mut(&mut list)[index] = x;
                Value::List(list)
            }
            InstKind::ListPush(v, x) => {
                let x = frame.get(*x).clone();
                let mut list = frame.take(*v).into_list();
                Arc::make_mut(&mut list).push(x);
                Value::List(list)
            }
            InstKind::ListTruncate(v, n) => {
                let n = frame.get(*n).i64();
                let mut list = frame.take(*v).into_list();
                let n = checked_index(n, list.len(), true).map_err(fail)?;
                Arc::make_mut(&mut list).truncate(n);
                Value::List(list)
            }
            InstKind::ListInsert(v, i, x) => {
                let (i, x) = (frame.get(*i).i64(), frame.get(*x).clone());
                let mut list = frame.take(*v).into_list();
                let index = checked_index(i, list.len(), true).map_err(fail)?;
                Arc::make_mut(&mut list).insert(index, x);
                Value::List(list)
            }
            InstKind::ListRemove(v, i) => {
                let i = frame.get(*i).i64();
                let mut list = frame.take(*v).into_list();
                let index = checked_index(i, list.len(), false).map_err(fail)?;
                Arc::make_mut(&mut list).remove(index);
                Value::List(list)
            }
            InstKind::MapNew(_) => Value::Map(Arc::default()),
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
                Arc::make_mut(&mut map).insert(k, x);
                Value::Map(map)
            }
            InstKind::MapSetValueAt(v, i, x) => {
                let (i, x) = (frame.get(*i).i64() as usize, frame.get(*x).clone());
                let mut map = frame.take(*v).into_map();
                Arc::make_mut(&mut map).entries[i].1 = x;
                Value::Map(map)
            }
            InstKind::MapRemoveAt(v, i) => {
                let i = frame.get(*i).i64() as usize;
                let mut map = frame.take(*v).into_map();
                Arc::make_mut(&mut map).remove_at(i);
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
            InstKind::CallRuntime(RuntimeFn::Exit, args) => {
                // Exit codes are the low 8 bits, as on POSIX systems.
                return Ok(Outcome::Exit((frame.get(args[0]).i64() & 0xff) as i32));
            }
            InstKind::CallC(c, _) => {
                return Err(fail(RuntimeError::CCall {
                    name: c.symbol.clone(),
                }));
            }
            InstKind::CallRuntime(rt, args) => {
                let args: Vec<&Value> = args.iter().map(|a| frame.get(*a)).collect();
                match call_runtime(*rt, &args, state).map_err(fail)? {
                    Some(v) => v,
                    None => continue,
                }
            }
            InstKind::MakeClosure(func, env) => {
                let env = match env {
                    Some(env) => frame.get(*env).clone(),
                    None => Value::List(Arc::default()),
                };
                Value::Closure(*func, Box::new(env))
            }
            InstKind::TaskSpawn(closure) | InstKind::TaskDetach(closure) => {
                let closure = frame.get(*closure).clone();
                let joinable = matches!(inst.kind, InstKind::TaskSpawn(_));
                let id = state
                    .start_task(module, limits, closure, joinable)
                    .map_err(fail)?;
                if !joinable {
                    continue;
                }
                Value::I64(id)
            }
            InstKind::TaskJoin(handle) => {
                let id = frame.get(*handle).i64();
                match state.join(id).map_err(fail)? {
                    Some(result) => result,
                    None => return Ok(Outcome::Stopped),
                }
            }
            InstKind::ChanNew(capacity) => {
                let capacity = frame.get(*capacity).i64();
                Value::I64(state.new_channel(capacity))
            }
            InstKind::ChanSend(chan, list) => {
                let (id, list) = (frame.get(*chan).i64(), frame.get(*list).clone());
                match state.send(id, list) {
                    Some(sent) => Value::Bool(sent),
                    None => return Ok(Outcome::Stopped),
                }
            }
            InstKind::ChanReceive(chan) => {
                let id = frame.get(*chan).i64();
                match state.receive(id) {
                    Some(Some(list)) => list,
                    Some(None) => Value::List(Arc::default()),
                    None => return Ok(Outcome::Stopped),
                }
            }
            InstKind::ChanClose(chan) => {
                let id = frame.get(*chan).i64();
                state.close_channel(id);
                continue;
            }
            InstKind::CallClosure(closure, args) => {
                let Value::Closure(callee, env) = frame.get(*closure) else {
                    unreachable!("validated IR: a closure");
                };
                let callee = *callee;
                let mut all = vec![(**env).clone()];
                all.extend(args.iter().map(|a| frame.get(*a).clone()));
                if stack.len() >= limits.max_call_depth {
                    return Err(fail(RuntimeError::StackOverflow {
                        limit: limits.max_call_depth,
                    }));
                }
                let new_frame = Frame::new(module, callee, all, inst.result, cache);
                stack.push(new_frame);
                continue;
            }
            InstKind::Call(callee, args) | InstKind::CallMut(callee, args) => {
                // `call.mut` moves its first argument, so the callee can
                // change it in place.
                let moves = matches!(inst.kind, InstKind::CallMut(..));
                let args: Vec<Value> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        if moves && i == 0 {
                            frame.take(*a)
                        } else {
                            frame.get(*a).clone()
                        }
                    })
                    .collect();
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
        UAdd => Value::I64(ops::uadd(a.i64(), b.i64())?),
        USub => Value::I64(ops::usub(a.i64(), b.i64())?),
        UMul => Value::I64(ops::umul(a.i64(), b.i64())?),
        UDiv => Value::I64(ops::udiv(a.i64(), b.i64())?),
        URem => Value::I64(ops::urem(a.i64(), b.i64())?),
        ULt => Value::Bool((a.i64() as u64) < (b.i64() as u64)),
        ULe => Value::Bool((a.i64() as u64) <= (b.i64() as u64)),
        UGt => Value::Bool((a.i64() as u64) > (b.i64() as u64)),
        UGe => Value::Bool((a.i64() as u64) >= (b.i64() as u64)),
        FAdd => float(a.f64() + b.f64()),
        FSub => float(a.f64() - b.f64()),
        FMul => float(a.f64() * b.f64()),
        FDiv => float(a.f64() / b.f64()),
        FRem => float(a.f64() % b.f64()),
        FEq => Value::Bool(a.f64() == b.f64()),
        FNe => Value::Bool(a.f64() != b.f64()),
        FLt => Value::Bool(a.f64() < b.f64()),
        FLe => Value::Bool(a.f64() <= b.f64()),
        FGt => Value::Bool(a.f64() > b.f64()),
        FGe => Value::Bool(a.f64() >= b.f64()),
    })
}

/// The `std.fs` operations: the result, or what failed and why.
fn fs_call(
    rt: RuntimeFn,
    cwd: &std::path::Path,
    args: &[&Value],
) -> Result<Value, (&'static str, std::io::Error)> {
    use std::io::ErrorKind;
    let path = cwd.join(args[0].str());
    match rt {
        RuntimeFn::FsList => {
            let mut names: Vec<String> = std::fs::read_dir(&path)
                .map_err(|e| ("list directory", e))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            Ok(str_list(names.into_iter().map(BrimStr::from).collect()))
        }
        RuntimeFn::FsCreateDir => {
            if path.is_dir() {
                return Ok(Value::Bool(false));
            }
            std::fs::create_dir_all(&path).map_err(|e| ("create directory", e))?;
            Ok(Value::Bool(true))
        }
        RuntimeFn::FsRemove | RuntimeFn::FsRemoveAll => {
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Value::Bool(false)),
                Err(e) => return Err(("remove", e)),
            };
            let removed = if !meta.is_dir() {
                std::fs::remove_file(&path)
            } else if rt == RuntimeFn::FsRemoveAll {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_dir(&path)
            };
            removed.map_err(|e| ("remove", e))?;
            Ok(Value::Bool(true))
        }
        RuntimeFn::FsRename => {
            std::fs::rename(&path, cwd.join(args[1].str())).map_err(|e| ("rename", e))?;
            Ok(Value::Bool(true))
        }
        RuntimeFn::FsSize => {
            let meta = std::fs::metadata(&path).map_err(|e| ("read the size of", e))?;
            Ok(Value::I64(meta.len() as i64))
        }
        RuntimeFn::FsModified => {
            let meta = std::fs::metadata(&path).map_err(|e| ("read the time of", e))?;
            let millis = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            Ok(Value::I64(millis))
        }
        RuntimeFn::FsAppend => {
            use std::io::Write;
            let text = args[1].str();
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(text.as_bytes()))
                .map_err(|e| ("append to", e))?;
            Ok(Value::I64(text.len() as i64))
        }
        _ => unreachable!("only std.fs operations"),
    }
}

/// Bytes as a `List[u8]`.
fn byte_list(data: &[u8]) -> Value {
    Value::List(Arc::new(
        data.iter().map(|b| Value::I64(i64::from(*b))).collect(),
    ))
}

/// A `List[u8]` as bytes.
fn bytes(list: &Value) -> Vec<u8> {
    list.list().iter().map(|b| b.i64() as u8).collect()
}

fn float(x: f64) -> Value {
    Value::F64(x.to_bits())
}

fn str_list(items: Vec<BrimStr>) -> Value {
    Value::List(Arc::new(items.into_iter().map(Value::Str).collect()))
}

/// Runs a socket operation, recording its error as the last error.
fn net<T>(
    state: &mut State<'_, '_, '_>,
    op: impl FnOnce(&brim_runtime::net::Sockets) -> Result<T, String>,
    failed: Value,
    ok: impl FnOnce(T) -> Value,
) -> Option<Value> {
    state.last_error.clear();
    // Output written so far reaches the terminal before the program waits.
    let _ = state.out().flush();
    Some(match op(&state.shared.sockets) {
        Ok(value) => ok(value),
        Err(e) => {
            state.last_error = e;
            failed
        }
    })
}

fn call_runtime(
    rt: RuntimeFn,
    args: &[&Value],
    state: &mut State<'_, '_, '_>,
) -> Result<Option<Value>, RuntimeError> {
    use brim_runtime as r;
    let io = |e: std::io::Error| RuntimeError::Io {
        message: e.to_string(),
    };
    let str_value = |s: BrimStr| Some(Value::Str(s));
    let s = |i: usize| args[i].str();
    Ok(match rt {
        RuntimeFn::PrintI64 => {
            r::print_i64(&mut **state.out(), args[0].i64()).map_err(io)?;
            None
        }
        RuntimeFn::PrintBool => {
            r::print_bool(&mut **state.out(), args[0].bool()).map_err(io)?;
            None
        }
        RuntimeFn::PrintU64 => {
            r::print_str(&mut **state.out(), &(args[0].i64() as u64).to_string()).map_err(io)?;
            None
        }
        RuntimeFn::AbsI64 => Some(Value::I64(ops::abs(args[0].i64())?)),
        RuntimeFn::AbsF64 => Some(float(args[0].f64().abs())),
        RuntimeFn::Sqrt => Some(float(args[0].f64().sqrt())),
        RuntimeFn::Floor => Some(float(args[0].f64().floor())),
        RuntimeFn::Ceil => Some(float(args[0].f64().ceil())),
        RuntimeFn::Round => Some(float(args[0].f64().round())),
        RuntimeFn::PowF64 => Some(float(args[0].f64().powf(args[1].f64()))),
        RuntimeFn::PowI64 => Some(Value::I64(ops::pow(args[0].i64(), args[1].i64())?)),
        RuntimeFn::U64ToStr => str_value(BrimStr::from((args[0].i64() as u64).to_string())),
        RuntimeFn::U64ToF64 => Some(float(args[0].i64() as u64 as f64)),
        RuntimeFn::PrintF64 => {
            r::print_str(&mut **state.out(), &r::float::format_f64(args[0].f64())).map_err(io)?;
            None
        }
        RuntimeFn::F64ToStr => str_value(BrimStr::from(r::float::format_f64(args[0].f64()))),
        RuntimeFn::I64ToF64 => Some(float(args[0].i64() as f64)),
        RuntimeFn::F64ToI64 => {
            let x = args[0].f64();
            // Inside `i64` after rounding toward zero; NaN fails both tests.
            if !(x > -9_223_372_036_854_775_809.0 && x < 9_223_372_036_854_775_808.0) {
                return Err(RuntimeError::FloatToInt);
            }
            Some(Value::I64(x as i64))
        }
        RuntimeFn::StrParseFloatOk => Some(Value::Bool(r::float::parse_f64(s(0)).is_some())),
        RuntimeFn::StrParseFloat => Some(float(r::float::parse_f64(s(0)).unwrap_or(0.0))),
        RuntimeFn::PrintStr => {
            r::print_str(&mut **state.out(), s(0)).map_err(io)?;
            None
        }
        RuntimeFn::EPrintStr => {
            // Keep stdout and stderr in order when both go to a terminal.
            state.out().flush().map_err(io)?;
            r::eprint_str(&mut **state.err(), s(0)).map_err(io)?;
            None
        }
        RuntimeFn::StrConcat => str_value(r::str_concat(s(0), s(1))),
        RuntimeFn::I64ToStr => str_value(r::i64_to_str(args[0].i64())),
        RuntimeFn::BoolToStr => str_value(r::bool_to_str(args[0].bool())),
        RuntimeFn::StrQuote => str_value(r::str_quote(s(0))),
        RuntimeFn::StrEq => Some(Value::Bool(r::str_eq(s(0), s(1)))),
        RuntimeFn::StrCompare => Some(Value::I64(r::str_compare(s(0), s(1)))),
        RuntimeFn::StrLen => Some(Value::I64(s(0).len() as i64)),
        RuntimeFn::StrContains => Some(Value::Bool(s(0).contains(s(1)))),
        RuntimeFn::StrStartsWith => Some(Value::Bool(s(0).starts_with(s(1)))),
        RuntimeFn::StrEndsWith => Some(Value::Bool(s(0).ends_with(s(1)))),
        RuntimeFn::StrFind => Some(Value::I64(r::str_find(s(0), s(1)))),
        RuntimeFn::StrSlice => str_value(r::str_slice(s(0), args[1].i64(), args[2].i64())?),
        RuntimeFn::StrTrim => str_value(r::str_trim(s(0))),
        RuntimeFn::StrTrimStart => str_value(r::str_trim_start(s(0))),
        RuntimeFn::StrTrimEnd => str_value(r::str_trim_end(s(0))),
        RuntimeFn::StrToUpper => str_value(r::str_to_upper(s(0))),
        RuntimeFn::StrToLower => str_value(r::str_to_lower(s(0))),
        RuntimeFn::StrReplace => str_value(r::str_replace(s(0), s(1), s(2))),
        RuntimeFn::StrRepeat => str_value(r::str_repeat(s(0), args[1].i64())),
        RuntimeFn::StrSplit => Some(str_list(r::str_split(s(0), s(1)))),
        RuntimeFn::StrLines => Some(str_list(r::str_lines(s(0)))),
        RuntimeFn::StrChars => Some(str_list(r::str_chars(s(0)))),
        RuntimeFn::StrCodePoints => Some(Value::List(Arc::new(
            s(0).chars()
                .map(|c| Value::I64(i64::from(u32::from(c))))
                .collect(),
        ))),
        RuntimeFn::CodePointOk => Some(Value::Bool(
            u32::try_from(args[0].i64())
                .ok()
                .and_then(char::from_u32)
                .is_some(),
        )),
        // Called next to `CodePointOk`, whose result decides which is used.
        RuntimeFn::CodePointToStr => str_value(BrimStr::from(
            u32::try_from(args[0].i64())
                .ok()
                .and_then(char::from_u32)
                .map_or(String::new(), String::from),
        )),
        RuntimeFn::ListSortI64 | RuntimeFn::ListSortU64 | RuntimeFn::ListSortStr => {
            let mut items: Vec<Value> = args[0].list().to_vec();
            match rt {
                RuntimeFn::ListSortI64 => items.sort_by_key(Value::i64),
                RuntimeFn::ListSortU64 => items.sort_by_key(|v| v.i64() as u64),
                _ => items.sort_by(|a, b| a.str().as_bytes().cmp(b.str().as_bytes())),
            }
            Some(Value::List(Arc::new(items)))
        }
        RuntimeFn::Sha256 => Some(byte_list(&r::crypto::sha256(&bytes(args[0])))),
        RuntimeFn::HmacSha256 => Some(byte_list(&r::crypto::hmac_sha256(
            &bytes(args[0]),
            &bytes(args[1]),
        ))),
        RuntimeFn::Md5 => Some(byte_list(&r::crypto::md5(&bytes(args[0])))),
        RuntimeFn::Pbkdf2Sha256 => Some(byte_list(&r::crypto::pbkdf2_sha256(
            &bytes(args[0]),
            &bytes(args[1]),
            args[2].i64(),
            args[3].i64(),
        ))),
        RuntimeFn::RandomBytes => {
            let n = usize::try_from(args[0].i64()).unwrap_or(0);
            Some(byte_list(&r::crypto::random_bytes(n)))
        }
        RuntimeFn::NetReadBytes => net(
            state,
            |n| n.read_bytes(args[0].i64(), args[1].i64()),
            Value::List(Arc::default()),
            |b| byte_list(&b),
        ),
        RuntimeFn::NetWriteBytes => {
            let data = bytes(args[1]);
            net(
                state,
                |n| n.write_bytes(args[0].i64(), &data),
                Value::I64(0),
                Value::I64,
            )
        }
        RuntimeFn::StrBytes => Some(Value::List(Arc::new(
            s(0).bytes().map(|b| Value::I64(i64::from(b))).collect(),
        ))),
        RuntimeFn::BytesUtf8Ok => Some(Value::Bool(std::str::from_utf8(&bytes(args[0])).is_ok())),
        // Called next to `BytesUtf8Ok`, whose result decides which is used.
        RuntimeFn::BytesToStr => str_value(BrimStr::from(
            String::from_utf8_lossy(&bytes(args[0])).as_ref(),
        )),
        RuntimeFn::StrParseIntOk => Some(Value::Bool(r::str_parse_int(s(0)).is_some())),
        RuntimeFn::StrParseInt => Some(Value::I64(r::str_parse_int(s(0)).unwrap_or(0))),
        RuntimeFn::ReadLine => {
            state.out().flush().map_err(io)?;
            let mut line = String::new();
            state.last_error.clear();
            match lock(&state.shared.input).read_line(&mut line) {
                Ok(0) => state.last_error = "end of input".into(),
                Ok(_) => {
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    state.last_error = "input is not valid UTF-8".into();
                }
                Err(_) => state.last_error = "cannot read input".into(),
            }
            str_value(BrimStr::from(line))
        }
        RuntimeFn::ReadFile => {
            state.last_error.clear();
            match std::fs::read_to_string(state.shared.cwd.join(s(0))) {
                Ok(text) => str_value(BrimStr::from(text)),
                Err(e) => {
                    state.last_error = r::file_error("read", s(0), &e);
                    str_value(BrimStr::from(""))
                }
            }
        }
        RuntimeFn::WriteFile => {
            state.last_error.clear();
            match std::fs::write(state.shared.cwd.join(s(0)), s(1)) {
                Ok(()) => Some(Value::I64(s(1).len() as i64)),
                Err(e) => {
                    state.last_error = r::file_error("write", s(0), &e);
                    Some(Value::I64(0))
                }
            }
        }
        RuntimeFn::LastError => str_value(BrimStr::from(state.last_error.as_str())),
        RuntimeFn::FsExists => Some(Value::Bool(state.shared.cwd.join(s(0)).exists())),
        RuntimeFn::FsIsDir => Some(Value::Bool(state.shared.cwd.join(s(0)).is_dir())),
        RuntimeFn::FsList
        | RuntimeFn::FsCreateDir
        | RuntimeFn::FsRemove
        | RuntimeFn::FsRemoveAll
        | RuntimeFn::FsRename
        | RuntimeFn::FsSize
        | RuntimeFn::FsModified
        | RuntimeFn::FsAppend => {
            state.last_error.clear();
            let cwd = state.shared.cwd.clone();
            match fs_call(rt, &cwd, args) {
                Ok(value) => Some(value),
                Err((verb, e)) => {
                    state.last_error = r::fs_error(verb, s(0), &e);
                    Some(match rt {
                        RuntimeFn::FsList => Value::List(Arc::default()),
                        RuntimeFn::FsSize | RuntimeFn::FsModified | RuntimeFn::FsAppend => {
                            Value::I64(0)
                        }
                        _ => Value::Bool(false),
                    })
                }
            }
        }
        RuntimeFn::ProcessRun => {
            let list = args[1].list();
            let argv: Vec<&str> = list.iter().map(Value::str).collect();
            match r::process_run(&state.shared.cwd, s(0), &argv) {
                Ok(out) => {
                    state.last_error.clear();
                    state.process = out;
                }
                Err(e) => {
                    state.last_error = e;
                    state.process.code = -1;
                    state.process.stdout.clear();
                    state.process.stderr.clear();
                }
            }
            Some(Value::I64(state.process.code))
        }
        RuntimeFn::ProcessStdout => str_value(BrimStr::from(state.process.stdout.as_str())),
        RuntimeFn::ProcessStderr => str_value(BrimStr::from(state.process.stderr.as_str())),
        RuntimeFn::Args => Some(str_list(
            state
                .shared
                .args
                .iter()
                .map(|a| BrimStr::from(a.as_str()))
                .collect(),
        )),
        RuntimeFn::EnvHas => Some(Value::Bool(std::env::var_os(s(0)).is_some())),
        RuntimeFn::EnvGet => str_value(BrimStr::from(std::env::var(s(0)).unwrap_or_default())),
        RuntimeFn::NetListen => net(
            state,
            |n| n.listen(s(0), args[1].i64()),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetAccept => net(
            state,
            |n| n.accept(args[0].i64()),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetNextReady => net(
            state,
            |n| n.next_ready(args[0].i64(), args[1].i64()),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetKeep => {
            state.shared.sockets.keep(args[0].i64(), args[1].i64());
            None
        }
        RuntimeFn::NetConnect => net(
            state,
            |n| n.connect(s(0), args[1].i64()),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetReadUntil => net(
            state,
            |n| n.read_until(args[0].i64(), s(1), args[2].i64()),
            Value::Str(BrimStr::from("")),
            |t| Value::Str(BrimStr::from(t)),
        ),
        RuntimeFn::NetReadExact => net(
            state,
            |n| n.read_exact(args[0].i64(), args[1].i64()),
            Value::Str(BrimStr::from("")),
            |t| Value::Str(BrimStr::from(t)),
        ),
        RuntimeFn::NetReadAll => net(
            state,
            |n| n.read_all(args[0].i64()),
            Value::Str(BrimStr::from("")),
            |t| Value::Str(BrimStr::from(t)),
        ),
        RuntimeFn::NetWrite => net(
            state,
            |n| n.write(args[0].i64(), s(1)),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetClose => {
            state.last_error.clear();
            state.shared.sockets.close(args[0].i64());
            None
        }
        RuntimeFn::NetLocalPort => net(
            state,
            |n| n.local_port(args[0].i64()),
            Value::I64(0),
            Value::I64,
        ),
        RuntimeFn::NetSetTimeout => {
            state.last_error.clear();
            if let Err(e) = state
                .shared
                .sockets
                .set_timeout(args[0].i64(), args[1].i64())
            {
                state.last_error = e;
            }
            None
        }
        RuntimeFn::UrlDecode => {
            state.last_error.clear();
            match brim_runtime::net::url_decode(s(0), args[1].bool()) {
                Ok(text) => str_value(BrimStr::from(text)),
                Err(e) => {
                    state.last_error = e;
                    str_value(BrimStr::from(""))
                }
            }
        }
        RuntimeFn::CharFromCode => {
            state.last_error.clear();
            let c = u32::try_from(args[0].i64()).ok().and_then(char::from_u32);
            match c {
                Some(c) => str_value(BrimStr::from(c.to_string())),
                None => {
                    state.last_error = "not a Unicode character".into();
                    str_value(BrimStr::from(""))
                }
            }
        }
        RuntimeFn::CharCode => Some(Value::I64(
            s(0).chars().next().map_or(-1, |c| i64::from(u32::from(c))),
        )),
        RuntimeFn::HttpRequest => {
            state.last_error.clear();
            let _ = state.out().flush();
            match https::request(s(0), s(1), s(2), s(3), args[4].i64()) {
                Ok(text) => str_value(BrimStr::from(text)),
                Err(e) => {
                    state.last_error = e;
                    str_value(BrimStr::from(""))
                }
            }
        }
        RuntimeFn::RandomSeed => {
            r::random::seed(args[0].i64());
            None
        }
        RuntimeFn::RandomRange => Some(Value::I64(r::random::range(args[0].i64(), args[1].i64()))),
        RuntimeFn::RandomFloat => Some(float(r::random::float())),
        RuntimeFn::TimeNowMillis => Some(Value::I64(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        )),
        RuntimeFn::TimeMonotonicMillis => {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let start = START.get_or_init(std::time::Instant::now);
            Some(Value::I64(
                i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX),
            ))
        }
        RuntimeFn::TimeSleepMillis => {
            let _ = state.out().flush();
            state.sleep(args[0].i64());
            None
        }
        RuntimeFn::UrlEncode => str_value(BrimStr::from(brim_runtime::net::url_encode(s(0)))),
        RuntimeFn::Exit => unreachable!("handled by the interpreter loop"),
        RuntimeFn::Assert => {
            if !args[0].bool() {
                return Err(RuntimeError::AssertionFailed { values: None });
            }
            None
        }
        RuntimeFn::AssertEqI64 | RuntimeFn::AssertEqBool | RuntimeFn::AssertEqStr => {
            if args[0] != args[1] {
                let show = |v: &Value| match v {
                    Value::I64(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Str(s) => r::shown_str(s),
                    other => unreachable!("validated IR: assert_eq on {other:?}"),
                };
                return Err(RuntimeError::AssertionFailed {
                    values: Some((show(args[0]), show(args[1]))),
                });
            }
            None
        }
    })
}

#[cfg(test)]
mod tests;
