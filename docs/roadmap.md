# Roadmap

Status legend: **done**, **in progress**, **planned**. Only items marked done exist in the code today.

## Phase 0: Repository and language design (done)

- Rust workspace, toolchain pin, CI, contribution and security policies.
- Initial language specification (`docs/language-spec.md`) with an EBNF grammar and documented open questions.
- Architecture and error-handling documents.

## Phase 1: Lexer and parser (done)

- Lexer with byte-offset spans, line/column mapping, string escapes, integer literals, reserved words, and recovery from invalid input.
- Recursive-descent parser with precedence climbing, newline-terminated statements, error recovery, and a nesting limit.
- Diagnostics with codes, labels, notes and help, rendered with source excerpts.

## Phase 2: Semantic analysis (done for the 0.1 language)

- Function table, lexical scopes, duplicate detection.
- Type checking for `i64`, `bool`, `string`, `void`, with no implicit conversions.
- Argument count and type checks, return checks, entry-point validation.
- Typo suggestions, unreachable-code and unused-value warnings.

## Phase 3: First executable backend (done, as a temporary interpreter)

- Brim IR: typed CFG, text form, validator.
- Lowering from the checked program to IR.
- Reference interpreter over IR, with checked arithmetic, call-depth limits and located run-time errors.
- `brim check`, `brim run`, `brim check --emit tokens|ast|ir`.

## Phase 3b: Control flow (done)

- `if` / `else` / `else if` (as statements; `if` as an expression is an open question).
- `while` loops with `break` and `continue`.
- Mutable variables (`let mut`), assignment and compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`).
- Divergence-based "returns on all paths" analysis and unreachable-code warnings; `while true` without `break` counts as never ending.
- The `to_string` builtin for `i64` and `bool`.

## Phase 4: Native code generation (done, first version)

- `LlvmBackend` behind the opt-in `llvm` Cargo feature, using `inkwell` 0.10 with LLVM 22 (see `architecture.md`, "Code generation").
- A small C runtime, embedded in the compiler and linked into every executable with the system C compiler.
- `brim build` produces an executable (`-o`, `--release` for LLVM's O2 pipeline). Projects build to `target/<package name>`.
- Run-time errors in native executables are byte-identical to the interpreter's, including the call stack and the call-depth limit.
- Differential testing: every integration program and example, debug and release, must match the interpreter exactly.
- A CI job with LLVM 22 on Linux and macOS; the frontend-only jobs stay LLVM-free.

Still open from this phase: debug information, Windows, cross-compilation, and linking without a C compiler.

**Current milestone: runtime and standard library (Phase 5).**

## Phase 5: Runtime and standard library (in progress)

- **Done:** memory management for strings in native executables (ARC, as recommended in `architecture.md`), with a runtime check mode used by the tests.
- Recoverable error handling in the language: errors as values the compiler makes you handle, with short propagation syntax (see `error-handling.md`).
- Structs, enums (sum types) with exhaustive `match`, collections, and string operations.
- An explicit optional type for "no value", instead of a null value.
- Standard library modules: I/O, files, formatting, environment, processes; later networking and HTTP for backend services.

## Phase 6: Package manager and project tooling (planned)

- Multi-file programs: modules, imports, visibility.
- `brim new`, `brim test`, `brim fmt`.
- `[dependencies]` in `brim.toml`, a lock file, dependency resolution, and later a package registry.
- API contracts for web frontends: declare API types once in Brim and generate the matching TypeScript types (and later a client) for Angular, so frontend and backend cannot drift apart.

## Phase 7: Editor support and production readiness (planned)

- Language server (diagnostics, go-to-definition, completion), reusing the compiler crates.
- Syntax highlighting definitions.
- Stability guarantees per edition, release process, binary distribution.
- Performance work, fuzzing, documentation site.

## Phase 8: Self-hosting (optional)

- Rewrite parts of the toolchain in Brim once the language and native backend are mature enough. This is optional and only worthwhile if it improves the project.
