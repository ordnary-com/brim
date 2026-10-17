# Concurrency

Status: built (first version), in the interpreter and native code.

## Goals

- A service handles requests at the same time: a slow request does not
  hold up the others.
- Programs can do independent work in parallel on several cores.
- No data races, by construction, and no new rules about ownership or
  borrowing to learn.
- The same behaviour in the interpreter (`brim run`) and in native code.

## The idea

Brim already has value semantics everywhere. Structs, lists, maps and sets
are values: changing one changes a copy (copy-on-write), a `fn` expression
copies what it captures, and `mut self` works on the caller's own variable.
Two parts of a program never see each other's changes; there is no shared
mutable memory.

So tasks can run on real threads and pass any value to each other. The
only thing that has to change is under the hood: the reference counts that
make copy-on-write cheap become atomic in native code, and the interpreter
uses `Arc` instead of `Rc`. A list sent to another task is shared until
one side changes it; then that side gets its own copy, as it does today.

Tasks talk through channels, and get results by joining. There is no
`async`/`await` and no colored functions: blocking calls block their own
task only.

## The API: `std.task`

```brim
import std.task

fn fib(n: i64) -> i64 {
    return if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    // Runs on its own thread; `join` waits and gives the result.
    let a = task.spawn(fn() -> i64 { return fib(30) })
    let b = task.spawn(fn() -> i64 { return fib(31) })
    print(a.join() + b.join())

    // A channel carries values of one type between tasks.
    let results: task.Channel[string] = task.channel()
    for name in ["a", "b", "c"] {
        task.detach(fn() {
            results.send(f"done: {name}")
        })
    }
    for _ in 0..3 {
        match results.receive() {
            Some(line) => print(line)
            None => {}
        }
    }
}
```

| Item | Meaning |
|------|---------|
| `spawn(f: fn() -> T) -> Task[T]` | Runs `f` on a new task. |
| `Task[T].join() -> T` | Waits for the task and returns its result. A task is joined once; joining again is a run-time error. |
| `detach(f: fn())` | Runs `f` on a new task that is never joined (a request handler, a background job). |
| `channel[T]() -> Channel[T]` | A new channel without a size limit. |
| `bounded[T](capacity: i64) -> Channel[T]` | A channel whose `send` waits while `capacity` values are queued. |
| `Channel[T].send(value: T) -> bool` | Queues a value; `false` when the channel is closed. |
| `Channel[T].receive() -> Option[T]` | The next value, waiting for one; `None` once the channel is closed and empty. |
| `Channel[T].close()` | No more values: senders get `false`, receivers drain what is queued and then get `None`. |

A `Channel` is a handle: copies of it (captured by a `fn`, sent over
another channel, stored in a struct) refer to the same channel.

### Rules

- A run-time error in any task (overflow, an index out of range, a failed
  `unwrap`) stops the whole program, with that task's call stack. Errors a
  program expects to handle are `Result` values, as everywhere else.
- `exit(code)` in any task ends the program.
- The program ends when `main` returns, also while detached tasks run.
- Output from `print` is written a line at a time, so lines of different
  tasks do not mix.
- A program that waits for a value no task will ever send waits forever.
  Closing channels when done is the way to end a loop over `receive`.

### Services

```brim
fn serve(port: i64) -> Result[bool, string] {
    let server = http.serve("0.0.0.0", port)?
    loop {
        let request = server.next()?
        task.detach(fn() {
            match request.respond(handle(request)) {
                Ok(_) => {}
                Err(e) => log.warn(e)
            }
        })
    }
}
```

`Server.next()` and `Request.respond` may be called from different tasks;
keep-alive connections go back to the server after the response.

## Implementation

**Checker.** `std.task` is written in Brim on top of intrinsics, like
`std.json`: `__task_spawn`, `__task_join`, `__task_detach`, `__chan_new`,
`__chan_send`, `__chan_receive` and `__chan_close`. The values cross as a
one-element `List[T]`, so neither backend needs to know their type:
`spawn` wraps `f` as `fn() -> List[T] { return [f()] }`, and `receive`
gets an empty list when the channel is closed.

**IR.** New instructions `TaskSpawn(closure)`, `TaskDetach(closure)`,
`TaskJoin(handle)`, `ChanNew(capacity)`, `ChanSend(chan, list)`,
`ChanReceive(chan)` and `ChanClose(chan)`. Task and channel handles are
`i64` values; a stale handle (a closed and drained channel that was freed)
behaves as a closed channel.

**Interpreter.** Values use `Arc`. Each task runs the instruction loop on
its own OS thread with its own stack; the module, standard output and
error, and the socket table are shared behind locks. The step limit
applies per task. On WebAssembly, where there are no threads, starting a
task is a run-time error.

**Native.** The runtime starts tasks with `pthread_create`
(`_beginthreadex` on Windows) and keeps channels as queues of list
pointers behind a mutex and condition variables. Reference counts use
atomic operations (`__atomic_*`); the per-thread state of the runtime (the
call stack for error messages, the last error) is thread-local. The
memory check mode (`BRIM_RT_CHECK=1`) counts live values atomically.

## Not in this version

- `select` over several channels, and `receive` with a timeout.
- Shared mutable state (a `Mutex[T]`): a task that owns the state and
  answers over channels covers it.
- Cancelling a task.
- Limiting the number of threads: every task is a thread. A pool of
  worker tasks that read from one `bounded` channel limits work.
