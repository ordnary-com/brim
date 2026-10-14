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
      ├──► brim-interp     reference interpreter      (works today: `brim run`)
      └──► brim-codegen    native backend interface   (LLVM: planned, not implemented)
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
| `brim-codegen` | `Backend` trait; `LlvmBackend` placeholder that reports "unavailable" | ir |
| `brim-manifest` | `brim.toml` parsing and validation (`M0xxx`) | diagnostics, serde, toml |
| `brim-cli` | The `brim` binary: argument parsing, commands, exit codes | everything above |

`brim-lower` and `brim-manifest` were added beyond the initially sketched layout. `brim-lower` keeps the IR independent of the AST and of semantic analysis: `brim-ir` itself depends on neither. `brim-manifest` keeps project configuration reusable by future tools, such as a package manager or language server, without depending on the CLI. `brim-interp` is separate from `brim-codegen` so the temporary interpreter is never confused with the native backend.

Only `brim-manifest` has third-party dependencies (`serde`, `toml`, pinned exactly in the workspace `Cargo.toml`). The compiler core is dependency-free.

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

`brim_ir::validate` checks: the entry function signature, slot and value types, single definition and same-block use of values, operand and result types of every instruction, call argument counts and types (including runtime calls), return types, and branch targets. Lowering is expected to always produce valid IR, so a validation failure is reported as an internal compiler error (exit code 70).

No optimization passes exist yet. They should be added only once there is a native backend to measure.

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

Planned native representation: `i64` as a 64-bit integer, `bool` as an 8-bit `0`/`1`, `str` as a pointer to a runtime-owned string object. The C-ABI (`extern "C"`) entry points will be added to `brim-runtime` together with the LLVM backend. They are deliberately not present yet: they would need `unsafe` code that nothing could call or test.

Integer arithmetic is **not** a runtime call. The backend emits the operation together with an overflow check, such as LLVM's `*.with.overflow` intrinsics, and branches to a trap. The semantics are defined once, in `brim_runtime::ops`, and the interpreter uses those functions directly.

## Memory management

**Current state.** The only heap-allocated values are strings. They are immutable. In the interpreter they are `Rc<str>`: cloning is cheap and memory is freed when the last reference goes away.

**Recommendation for the native backend: automatic reference counting (ARC) of immutable heap values.** For now that means strings only.

| Option | For | Against |
|--------|-----|---------|
| ARC (recommended) | Deterministic and predictable latency (important for services). No collector thread. Simple runtime. Easy C interop. In 0.1, values are immutable and there are no aggregate types, so **reference cycles are impossible** and ARC is complete. | Refcount traffic costs throughput, and atomic counts would be needed once threads exist. Cycles become possible once mutable aggregates are added, which would need weak references, a cycle collector, or language rules that prevent cycles. |
| Tracing GC | Handles cycles. High allocation throughput. | Pause times, a much larger runtime, and harder FFI. It needs stack maps from the backend. It is the kind of elaborate runtime this phase should avoid. |
| Ownership / borrowing | Zero runtime cost and strong guarantees. | It is a large language-design commitment, explicitly out of scope for the first implementation, and it would dominate the language's learning curve. |
| Arenas / leak-everything | Trivial to implement. | Unbounded memory growth makes it unsuitable for long-running servers, a primary use case. |

ARC keeps the runtime small, matches the use cases (long-running servers favor predictable memory over peak throughput), and leaves room to add escape analysis (stack allocation) or ownership-style optimizations later without changing language semantics. This decision should be revisited before aggregate types (structs, collections) are added, because that is when cycles become possible.

**Safety claims.** Brim 0.1 programs cannot express pointers, manual memory management or out-of-bounds access, and the interpreter is written in safe Rust. That is a property of today's tiny language, **not a memory-safety guarantee** of the future native implementation. Native code generation and FFI will need their own explicit safety design.

## Code generation plan (LLVM)

Findings from the environment used to set up the project (October 2026):

- Rust stable 1.98 (via rustup). The workspace's minimum supported Rust version is 1.88 (edition 2024 and let-chains).
- LLVM 22.1.8 and 23.1.3 are available through Homebrew on the development machine.
- `inkwell` 0.10.0, a maintained safe wrapper over `llvm-sys`, supports LLVM up to **22**. LLVM 23 is not supported yet.

Plan:

1. Implement `LlvmBackend` in `brim-codegen` behind a non-default Cargo feature (for example `llvm`), using `inkwell` with its `llvm22-1` feature. Without the feature, `brim build` keeps reporting the backend as unavailable. The frontend, its tests and the default CI job must never need LLVM.
2. Lower each IR function directly: slots become `alloca`s in the entry block, values become LLVM SSA values, blocks map one to one, checked arithmetic uses `llvm.*.with.overflow` plus a branch to a trap, and runtime calls become calls to the `brim_rt_*` symbols.
3. Build `brim-runtime` as a static library with `extern "C"` entry points and link it with the system linker (invoked through `cc`, or `clang` where available).
4. Add a CI job that installs LLVM 22 and runs differential tests: every `tests/integration` program must produce identical output in the interpreter and as a native executable.

Supported LLVM versions and platforms will be documented here once the backend exists. Until then, **Brim has no native code generation**.

## Decisions and trade-offs

| Decision | Rationale | Revisit when |
|----------|-----------|--------------|
| Hand-written lexer and parser, no generator | Best control over error messages and recovery. The grammar is small. | Grammar grows much larger |
| Newline-terminated statements | Requested: no semicolons in ordinary code. Continuation after operators keeps it predictable. | Method chains (`.` at line start) |
| Separate checked tree instead of annotating the AST | Later phases cannot accidentally use unchecked data, and the AST stays a pure syntax model for future tooling such as formatters. | Never, probably |
| Block-local IR values plus slots | Trivial validation and interpretation, and maps directly onto `alloca` + `mem2reg`. | An optimizer needs real SSA |
| Iterative interpreter with a heap-allocated call stack | Deep Brim recursion produces a clean `R0003` error instead of crashing the host. | Never |
| Checked integer arithmetic | Silent wraparound is a common source of bugs in server software. | Performance data from the native backend |
| No `brim build` output yet, but the command exists | `build` validates the IR and then reports the missing backend clearly (exit 4) instead of pretending. | LLVM backend lands |
| `toml` + `serde` for the manifest | TOML parsing is complex and well solved. Versions are pinned. | Never |
