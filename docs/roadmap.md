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

- **Done:** debug information (DWARF line tables, functions and simple variables for lldb and gdb) and native builds on Windows (the runtime through Winsock, Win32 and WinHTTP; linking with `clang`).
- **Done:** calling C: `extern fn` declarations, `[build] c-sources` and `[build] link`.

Still open from this phase: cross-compilation, and linking without a C compiler.

**Current milestone: the road to 1.0** (below). Concurrency is built (`std.task`).

## Phase 5: Runtime and standard library (in progress)

- **Done:** memory management for strings in native executables (ARC, as recommended in `architecture.md`), with a runtime check mode used by the tests.
- **Done:** structs: declarations, literals, field access and field assignment, with value semantics. Struct equality and printing are still open.
- **Done:** enums with payloads and exhaustive `match` (on enums, integers, strings and booleans).
- **Done:** generics: generic functions, structs and enums with inferred type arguments, compiled by monomorphization.
- **Done:** `Option[T]` and `Result[T, E]` as built-in enums.
- **Done:** recoverable error handling: `Result` values with the `?` operator (also for `Option`). Error type conversion is still open.
- **Done:** `List[T]` with literals, indexing, methods and copy-on-write value semantics, and `for` loops over lists and ranges.
- **Done:** `Map[K, V]` with `i64`, `bool` or `string` keys, insertion-ordered, with the same value semantics.
- **Done:** methods in `impl` blocks, with `self`, `mut self` and type-level methods, also for generic types.
- **Done:** a first standard library: string methods and comparison, standard input and error, files, command-line arguments, environment variables and exit codes.
- **Done:** standard library modules for backend services: `std.fs`, `std.process`, `std.time`, `std.log`, `std.random`, `std.net`, `std.http` (client, server with keep-alive, API functions), `std.json` (with typed `encode` and `decode`), `std.postgres`, `std.crypto` and `std.base64`.
- **Done:** concurrency: tasks and channels (`std.task`, `docs/design/concurrency.md`), on threads in the interpreter and in native code, and HTTP servers that handle requests at the same time.
- Open: TLS for `std.postgres` and HTTP servers; `select` over channels and `receive` with a timeout.

## Phase 6: Package manager and project tooling (done, first version)

- **Done:** multi-file programs: every file is a module, `import` with aliases and item lists, `pub` visibility, diagnostics across files.
- **Done:** `brim new`, and `brim test` with `assert` and `assert_eq`.
- **Done:** `brim fmt`, a token-based formatter that keeps comments and line breaks.
- **Done:** `[dependencies]` from directories and git repositories, `brim.lock`, `brim update`.
- **Done:** version requirements with semver resolution (one version per package in a project, conflicts explained), recorded with checksums in `brim.lock`.
- **Done:** a package registry (`registry/`, protocol in `docs/registry.md`) with `brim publish`, `brim login` and `brim yank`.
- **Done:** workspaces: `[workspace] members`, one `brim.lock`, `-p NAME`.
- Open: accounts for publishers through Ordnary's identity service (tokens are issued by an administrator for now).
- **Done:** API contracts for web frontends: `brim gen ts` writes TypeScript types for a program's `pub` types.
- **Done:** `json.encode` and `json.decode` with the same mapping, `serve_api(module, request)` for a module's `pub fn`s, and `brim gen ts --angular` for the matching Angular client.

## Phase 7: Editor support and production readiness (done, first version)

- **Done:** a language server, `brim lsp`: diagnostics for every file of the program, formatting, outline, go to definition and hover; completion (names, `module.` and the members of a value's type), find references, rename, signature help and quick fixes from the diagnostics.
- **Done:** editor support for VS Code, Neovim, Vim, Helix, Zed, Emacs, Sublime Text, JetBrains IDEs, Kate, Kakoune, nano, micro and Notepad++ (`editors/`), with a tree-sitter grammar and a TextMate grammar.
- **Done:** stability rules per edition, a release process and release binaries for Linux, macOS and Windows (`docs/releasing.md`).
- **Done:** fuzzing: a mutation fuzzer in the test suite (frontend, formatter, interpreter) and cargo-fuzz targets run weekly.
- **Done:** a documentation site (`site/`, built with mdBook): introduction, getting started, a tour, projects and tools, and the reference documents. Its examples run in the tests.
- **Done:** benchmarks and performance work (`benches/`), incremental builds with a cache, `brim doc`, and the interpreter as WebAssembly.

## The road to 1.0

- **Done:** the specification describes the whole language, and its open questions are decided (`language-spec.md`, sections 11 and 12).
- **Done:** the edition policy and what 1.0 keeps stable (`releasing.md`).
- **Done:** a security review of the runtime, the package client and the registry, with its findings fixed or recorded (`design/security-review.md`).
- Release candidates (`1.0.0-rc.N`), then 1.0.0.

## Phase 8: Self-hosting (optional)

- Rewrite parts of the toolchain in Brim once the language and native backend are mature enough. This is optional and only worthwhile if it improves the project.
