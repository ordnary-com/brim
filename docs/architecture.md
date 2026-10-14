# Architecture

This document describes how the Brim compiler is structured, and why. The language itself is specified in [`language-spec.md`](language-spec.md).

## Pipeline

```text
 source text (.brim)
      │  brim-lexer        tokens + lexical diagnostics
      ▼
   tokens
      │  brim-parser       recursive descent + precedence climbing
      ▼
    AST  (brim-ast)        untyped, mirrors the source, carries spans
      │  brim-semantic     name resolution, type checking, entry point
      ▼
 checked program           typed, name-resolved tree; no error types left
      │  brim-lower
      ▼
  Brim IR (brim-ir)        typed CFG; validated by brim_ir::validate
      ├──► brim-interp     reference interpreter      (`brim run`)
      └──► brim-codegen    LLVM backend + C runtime   (`brim build`, opt-in `llvm` feature)
```

Every stage reports problems as `brim_diagnostics::Diagnostic`s. A stage only runs if the previous stage produced no errors. Warnings never block compilation.

## Crates

| Crate | Responsibility | Depends on |
|-------|----------------|------------|
| `brim-diagnostics` | `SourceFile`, `Span`, `Location`, `Diagnostic`, terminal rendering | none |
| `brim-lexer` | UTF-8 text to `Token`s; lexical errors (`E00xx`) | diagnostics |
| `brim-ast` | Syntax tree types; fully parenthesized `Display` for tests | diagnostics |
| `brim-parser` | Tokens to AST; syntax errors (`E01xx`); error recovery | lexer, ast, diagnostics |
| `brim-runtime` | Runtime ABI (`RuntimeFn`, `AbiType`), checked arithmetic, print and string functions, `RuntimeError` (`R0xxx`) | none |
| `brim-stdlib` | Catalogue of builtin functions and their overloads, each mapped to a `RuntimeFn` | runtime |
| `brim-semantic` | Symbol tables, scopes, type checking (`E02xx`, `W0xxx`); produces `CheckedProgram` | ast, diagnostics, runtime, stdlib |
| `brim-ir` | IR data structures, text form, validator | diagnostics (spans), runtime (`RuntimeFn`) |
| `brim-lower` | `CheckedProgram` to IR | ir, runtime, semantic |
| `brim-interp` | Reference interpreter for IR | diagnostics, ir, runtime |
| `brim-codegen` | `Backend` trait; `LlvmBackend` (IR to LLVM IR, object emission, linking with the C runtime in `runtime/brim_rt.c`). LLVM code is only compiled with the `llvm` feature | diagnostics, ir, runtime; `inkwell` (optional) |
| `brim-manifest` | `brim.toml` parsing and validation (`M0xxx`) | diagnostics, serde, toml |
| `brim-cli` | The `brim` binary: argument parsing, commands, exit codes | everything above |

`brim-lower` and `brim-manifest` were added beyond the initially sketched layout. `brim-lower` keeps the IR independent of the AST and of semantic analysis: `brim-ir` itself depends on neither. `brim-manifest` keeps project configuration reusable by future tools, such as a package manager or language server, without depending on the CLI. `brim-interp` is separate from `brim-codegen` so the temporary interpreter is never confused with the native backend.

Only `brim-manifest` (`serde`, `toml`) and, with the `llvm` feature, `brim-codegen` (`inkwell`, which brings `llvm-sys`) have third-party dependencies, all pinned exactly in the workspace `Cargo.toml`. The compiler frontend is dependency-free.

## Source positions

- `Span { start, end }` stores **byte offsets** into the UTF-8 source. Spans are two integers, so they are cheap to copy into every token and AST node.
- Line and column are **computed on demand** by `SourceFile::location`, which binary-searches a table of line-start offsets. That costs O(log lines) per lookup, and lookups only happen when rendering diagnostics.
- Lines and columns are 1-based. Columns count Unicode scalar values (`char`s): in `"é" x`, `x` is at byte offset 5 but column 5, and `é` counts once.
- Columns do **not** account for grapheme clusters or terminal display width (for example East Asian wide characters or emoji), so carets may be misaligned on such lines. Tabs are expanded to 4 columns when rendering, both in the source line and under it, so carets stay aligned.

## Lexer

A hand-written single-pass scanner. Newlines are emitted as tokens because they terminate statements. Invalid characters produce an `Unknown` token after reporting `E0001`, so the parser can skip them without reporting a second, confusing error. The lexer never panics: every malformed input produces a diagnostic and lexing continues.

## Parser

Recursive descent for items and statements, and precedence climbing for binary expressions (`BinaryOp::precedence`).

- **Newline handling.** Inside parentheses a depth counter makes the parser skip newline tokens. After a binary operator, `=` or `->`, newlines are skipped explicitly.
- **Error recovery.** On an error the parser records a diagnostic and skips to the next statement boundary (newline, `;` or the enclosing `}`), or to the next `fn` at the top level. An unclosed block is detected when a `fn` keyword or end of file is reached.
- **Nesting limit.** Blocks and expressions together are limited to 128 levels of nesting (`E0105`). Long left-associative chains and `else if` chains count too, because later phases recurse over the tree. In debug builds the parser uses about 7 KB of stack per nesting level, so this limit keeps every phase within the 2 MB default thread stack used by tests. The `brim` binary also runs the compiler on a thread with a 64 MB stack.

## Semantic analysis

Two passes:

1. **Collect** every function signature into a function table, so declaration order does not matter.
2. **Check** each body with a stack of scopes (`Vec<HashMap<String, LocalId>>`).

Types are a small enum (`i64`, `bool`, `string`, `void`) plus an internal `Error` type. Any expression involving `Error` is not reported again, which prevents cascades: one undefined variable in a long expression produces exactly one error.

The output is a **checked program**: a typed tree where every name is resolved to a `LocalId` or `FnId`, every builtin call is resolved to a specific runtime function, and every operator is resolved to a type-specific operation (`IntAdd`, `StrConcat`, ...). Later phases never redo lookup or type inference.

"Returns on all paths" is computed structurally, without building a control-flow graph: each statement reports whether it *diverges* (control never reaches the next statement). `return`, `break` and `continue` diverge; `if`/`else` diverges when both branches do; `while true` diverges when its body has no `break`. A non-`void` function body must diverge. The same flag drives the unreachable-code warning. The checker keeps a stack of enclosing loops to validate `break`/`continue` and to record whether a loop contains a `break`.

Mutability is tracked per local: assignments to non-`mut` locals are rejected, and `mut` locals that are never reassigned produce a warning. Compound assignments are expanded in the checked tree (`x += 1` becomes `x = x + 1`) so lowering has a single assignment form.

## Intermediate representation

Brim IR is a typed control-flow graph, designed to be easy to validate, to interpret, and to translate to LLVM.

- A `Module` holds `Function`s and an entry `FuncId`.
- A `Function` has **slots** (typed local storage, like LLVM `alloca`; parameters are the first `param_count` slots), **values** (SSA-style temporaries), and **blocks**.
- Each `Block` holds instructions and exactly one `Terminator` (`ret`, `jump`, `branch`, `unreachable`).
- A value is defined exactly once and may only be used **later in the same block**. Data crosses block boundaries only through slots. This avoids the need for phi nodes or dominance analysis in the validator and interpreter. LLVM's `mem2reg` pass turns slots back into SSA registers, so nothing is lost in native code.
- Operations are fully typed (`iadd`, `ieq`, `bnot`). String operations and printing are calls into the runtime (`call @brim_rt_str_concat(...)`).
- Instructions carry an optional source span, used for run-time error locations today and for debug information later.

`&&` and `||` lower to a branch: the left value is stored in a temporary `bool` slot, the right side is evaluated in its own block only when needed, and a join block loads the result.

`if` lowers to a `branch` into a then block and an else block (empty when there is no `else`), both jumping to a join block. If neither branch falls through, the join block keeps no predecessors and an `unreachable` terminator. `while` lowers to a header block that evaluates the condition and branches to the body or the exit block; the body jumps back to the header. `break` jumps to the exit block and `continue` to the header. Mutable variables are ordinary slots, so loops need no phi nodes.

Text form (`brim check --emit ir`):

```text
; entry: main

fn add(s0 a: i64, s1 b: i64) -> i64 {
bb0:
  v0 = load s0
  v1 = load s1
  v2 = iadd v0, v1
  ret v2
}

fn main() {
  slot s0 result: i64
bb0:
  v0 = const i64 10
  v1 = const i64 32
  v2 = call add(v0, v1)
  store s0, v2
  v3 = load s0
  call @brim_rt_print_i64(v3)
  ret
}
```

Structs are aggregate values in the IR: `Module::structs` lists every struct type (`%0`, `%1`, ...) with its fields, and three instructions work on them: `make %N(v...)` builds a struct from one value per field, `extract v.i` reads field `i`, and `insert v.i, x` returns a copy of `v` with field `i` replaced. A field assignment `p.a.b = x` lowers to loading `p`, extracting `a`, inserting `x` into it, inserting the result into `p`, and storing `p` again. Struct types never contain themselves, so every struct has a fixed layout. The interpreter represents a struct as a shared, immutable `Rc<[Value]>`, and the LLVM backend as a named LLVM struct type, built with `insertvalue` and read with `extractvalue`.

`brim_ir::validate` checks: struct field types (no `void`, no struct containing itself), the entry function signature, slot and value types, single definition and same-block use of values, operand and result types of every instruction, call argument counts and types (including runtime calls), return types, and branch targets. Lowering is expected to always produce valid IR, so a validation failure is reported as an internal compiler error (exit code 70).

Brim IR has no optimization passes of its own. `brim build --release` runs LLVM's standard `default<O2>` pipeline on the generated code; IR-level passes should only be added when measurements show LLVM cannot do the job.

## Runtime ABI

Generated code reaches the outside world only through runtime functions listed in `brim_runtime::RuntimeFn`:

| Symbol               | Signature            | Purpose |
|----------------------|----------------------|---------|
| `brim_rt_print_i64`  | `(i64) -> void`      | `print(i64)` |
| `brim_rt_print_bool` | `(bool) -> void`     | `print(bool)` |
| `brim_rt_print_str`  | `(str) -> void`      | `print(string)` |
| `brim_rt_str_concat` | `(str, str) -> str`  | string `+` |
| `brim_rt_str_eq`     | `(str, str) -> bool` | string `==` / `!=` |
| `brim_rt_i64_to_str` | `(i64) -> str`       | `to_string(i64)` |
| `brim_rt_bool_to_str`| `(bool) -> str`      | `to_string(bool)` |

Native representation: `i64` is a 64-bit integer, `bool` an 8-bit `0`/`1`, and `str` a pointer to a `BrimStr` object: a 64-bit reference count (`-1` for static objects such as literals), a 64-bit byte length, then the UTF-8 bytes (not NUL-terminated).

The interpreter implements these functions with `brim-runtime`'s Rust functions. Native executables link a small C implementation of the same symbols, `crates/brim-codegen/runtime/brim_rt.c`, which the compiler embeds and compiles at link time. The two must behave identically; the differential tests enforce that (see [Code generation](#code-generation-llvm)).

Besides the functions above, the C runtime has backend-internal entry points that are not part of the IR: `brim_rt_enter(name, ...)` and `brim_rt_leave()` bracket every Brim call to maintain the call stack and enforce the call-depth limit (`brim_runtime::MAX_CALL_DEPTH`, shared with the interpreter), `brim_rt_trap(...)` reports a run-time error and exits with 101, and `brim_rt_retain(str)` / `brim_rt_release(str)` manage string memory (see [Memory management](#memory-management)). The runtime also defines the process entry point `main`, which calls the generated `brim_entry`.

Integer arithmetic is **not** a runtime call. The backend emits the operation together with an overflow check (LLVM's `llvm.s{add,sub,mul}.with.overflow` intrinsics, and explicit checks for division by zero and `i64::MIN / -1`), and branches to a trap. The semantics are defined once, in `brim_runtime::ops`, and the interpreter uses those functions directly.

## Memory management

**Current state.** The only heap-allocated values are strings. They are immutable. In the interpreter they are `Rc<str>`: cloning is cheap and memory is freed when the last reference goes away. Native executables use **automatic reference counting**, as recommended below:

- Every `BrimStr` carries a reference count. String literals are static objects with count `-1`, which retain and release ignore. Strings from concatenation and `to_string` start at 1 and are freed when the count reaches 0.
- The backend inserts the counting; the IR has no ownership information. The rules apply to every value that **contains** strings, including structs with string fields (counting a struct counts each string in it): every **slot** owns one reference (slots start as null; a store retains the new value, then releases the old one; every return releases all string slots). Every non-constant string **value** is owned by its block: loads retain, calls return strings at +1, and the block releases them just before its terminator. Since IR values never outlive their block, that is always after their last use. **Parameters** are borrowed: the caller keeps its reference during the call and the callee retains the parameter into its slot. A **returned** string is retained once more, so the caller receives it at +1.
- This is correct by construction but not minimal: a load followed by a call costs a retain and a release. Eliminating redundant pairs is an optimization for later.
- Counts are not atomic; Brim has no threads.
- With `BRIM_RT_CHECK=1` in the environment, the runtime checks memory instead of freeing it: retaining or releasing a freed string aborts the program, and strings still allocated when `main` returns are reported as a leak (exit code 102). The differential tests run every native program this way.

**Recommendation for the native backend: automatic reference counting (ARC) of immutable heap values.** For now that means strings only.

| Option | For | Against |
|--------|-----|---------|
| ARC (recommended) | Deterministic and predictable latency (important for services). No collector thread. Simple runtime. Easy C interop. In 0.1, values are immutable and there are no aggregate types, so **reference cycles are impossible** and ARC is complete. | Refcount traffic costs throughput, and atomic counts would be needed once threads exist. Cycles become possible once mutable aggregates are added, which would need weak references, a cycle collector, or language rules that prevent cycles. |
| Tracing GC | Handles cycles. High allocation throughput. | Pause times, a much larger runtime, and harder FFI. It needs stack maps from the backend. It is the kind of elaborate runtime this phase should avoid. |
| Ownership / borrowing | Zero runtime cost and strong guarantees. | It is a large language-design commitment, explicitly out of scope for the first implementation, and it would dominate the language's learning curve. |
| Arenas / leak-everything | Trivial to implement. | Unbounded memory growth makes it unsuitable for long-running servers, a primary use case. |

ARC keeps the runtime small, matches the use cases (long-running servers favor predictable memory over peak throughput), and leaves room to add escape analysis (stack allocation) or ownership-style optimizations later without changing language semantics. This decision should be revisited before aggregate types (structs, collections) are added, because that is when cycles become possible.

**Safety claims.** Brim 0.1 programs cannot express pointers, manual memory management or out-of-bounds access, and the interpreter is written in safe Rust. That is a property of today's tiny language, **not a memory-safety guarantee** of the native implementation: native executables run generated code and a C runtime. The runtime is small and only handles strings whose lengths the generated code never changes, but FFI and richer runtime types will need their own explicit safety design.

## Code generation (LLVM)

`brim build` compiles a program to a native executable. The LLVM backend lives in `brim-codegen` behind the non-default Cargo feature `llvm`, so the frontend, its tests and the default CI jobs never need LLVM. Without the feature, `brim build` checks the program and reports the backend as unavailable (exit 4).

**Supported toolchain.** LLVM 22, through `inkwell` 0.10 (its `llvm22-1` feature; LLVM 23 is not supported by `inkwell` yet). Linking uses the system C compiler: `$BRIM_CC`, else `$CC`, else `cc`. Tested on Linux (x86-64) and macOS (arm64) in CI. Windows is not supported yet: the runtime assumes a C11 compiler with the `cc` command-line interface.

**Lowering.** Each IR function is translated directly:

- Every function becomes an internal LLVM function `brim.<name>`; only `brim_entry` and `brim_max_call_depth` are exported, for the runtime's `main`.
- Slots become `alloca`s in a dedicated entry block, which also stores the parameters and jumps to IR block 0. LLVM's `mem2reg` (part of the release pipeline) turns them into registers.
- Values become SSA values, and IR blocks map one to one onto LLVM blocks. A checked operation splits its block: the failure edge goes to a block that calls `brim_rt_trap`.
- `bool` is `i1` inside functions and is widened to `i8` at runtime calls, as the ABI specifies.
- String literals become constant `BrimStr` objects (identical literals are shared).

**Run-time errors.** A native executable reports a run-time error exactly like `brim run`: the same message, code, source excerpt and call-stack notes, on stderr, with exit code 101. Everything except the call stack is known at compile time, so the backend renders the diagnostic for each trap site (`trap.rs`) and embeds the text. The runtime prints it and appends the call stack it tracks through `brim_rt_enter` / `brim_rt_leave`. Colors are never used.

**Executables.** The object file is written to a temporary directory together with the runtime source, and both are compiled and linked in one C compiler invocation (`-O0` for debug builds, `-O2` with `--release`). Code is generated for the host's generic CPU, so executables also run on other machines of the same architecture and OS.

**Testing.** With the feature enabled, `crates/brim-cli/tests/fixtures.rs` builds every `tests/integration` program and every example natively, in debug and release mode, and requires stdout, stderr and the exit code to be identical to the interpreter's. A separate CI job installs LLVM 22 and runs the whole test suite with `--features brim-cli/llvm`.

**Not yet done:** debug information (DWARF), cross-compilation, Windows, and avoiding the C compiler by linking with the system linker directly.

## Decisions and trade-offs

| Decision | Rationale | Revisit when |
|----------|-----------|--------------|
| Hand-written lexer and parser, no generator | Best control over error messages and recovery. The grammar is small. | Grammar grows much larger |
| Newline-terminated statements | Requested: no semicolons in ordinary code. Continuation after operators keeps it predictable. | Method chains (`.` at line start) |
| Separate checked tree instead of annotating the AST | Later phases cannot accidentally use unchecked data, and the AST stays a pure syntax model for future tooling such as formatters. | Never, probably |
| Block-local IR values plus slots | Trivial validation and interpretation, and maps directly onto `alloca` + `mem2reg`. | An optimizer needs real SSA |
| Iterative interpreter with a heap-allocated call stack | Deep Brim recursion produces a clean `R0003` error instead of crashing the host. | Never |
| Checked integer arithmetic | Silent wraparound is a common source of bugs in server software. | Performance data from the native backend |
| LLVM behind an opt-in Cargo feature | Contributors and the default CI jobs need no LLVM installation; the frontend can never accidentally depend on it. | LLVM is easy to install everywhere, or Brim ships prebuilt binaries |
| Native runtime in C, compiled at link time | Native code needs C-ABI entry points. In Rust they would need `unsafe` code (forbidden in the workspace) and a separately built static library that `brim` must find at run time. A few dozen lines of C embedded in the compiler need neither; the system C compiler is needed for linking anyway. | The runtime grows (ARC, I/O, networking), or Brim links without a C compiler |
| Run-time error text rendered at compile time | Native and interpreted programs print byte-identical errors, which makes differential testing exact, and the runtime needs no source files or debug information. Costs some binary size per trap site. | Binary size matters, or debug information lands |
| Call stack tracked by runtime calls | Gives the same call-stack notes and the same `R0003` depth limit as the interpreter, without unwinding or debug information. Costs two cheap calls per Brim call. | Performance data shows the cost matters |
| `toml` + `serde` for the manifest | TOML parsing is complex and well solved. Versions are pinned. | Never |
