# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- Native code generation: `brim build` compiles a program with LLVM 22 and links it with a small C runtime into a native executable. Opt-in at compile time with the `llvm` Cargo feature (`cargo build --features brim-cli/llvm`); without it, `brim build` still reports the backend as unavailable.
- `brim build -o <path>` and `brim build --release`. Projects build to `target/<package name>`.
- Native executables report run-time errors exactly like `brim run`, including the call stack and the 10,000-call depth limit.
- Differential tests that build every integration program and example natively (debug and release) and compare stdout, stderr and exit code with the interpreter; a CI job with LLVM 22 on Linux and macOS.
- Integration programs for `i64::MIN / -1` and negation overflow.
- Reference counting for strings in native executables: strings are freed when their last reference goes away (a loop building 5 million strings now peaks at under 2 MB instead of over 300 MB).
- `BRIM_RT_CHECK=1` runtime check mode for native executables: use after free aborts and leaks are reported. The differential tests run every program with it.
- Control flow: `if` / `else` / `else if`, `while` loops, `break` and `continue`.
- Mutable variables with `let mut`, assignment and compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`).
- `to_string` builtin for `i64` and `bool`.
- Divergence-based return checking: `if`/`else` where both branches return, and `while true` without `break`, count as never reaching the end.
- New diagnostics: `E0217` (`break`/`continue` outside a loop), `E0218` (assignment to an immutable variable), `W0003` (unneeded `mut`).
- `examples/control_flow.brim`.
- Initial Rust workspace for the Brim toolchain.
- Language specification draft for Brim 0.1 (edition 2026), with an EBNF grammar.
- Lexer with UTF-8 spans, string escapes, integer literals, reserved keywords and error recovery.
- Recursive-descent parser with precedence climbing, newline-terminated statements, error recovery and a nesting limit.
- Semantic analysis: scopes, function table, type checking for `i64`, `bool`, `string` and `void`, entry-point validation, typo suggestions and warnings.
- Diagnostics system with error codes, labels, notes, help and colored rendering.
- Brim IR with a text form and validator, and lowering from the checked program.
- Reference interpreter with checked integer arithmetic and located run-time errors.
- Native backend interface (`brim-codegen`). The LLVM backend is not implemented, and `brim build` reports it as unavailable.
- Runtime ABI definition and builtin catalogue (`print`).
- `brim.toml` manifest parsing and validation.
- `brim` CLI: `check` (with `--emit tokens|ast|ir`), `run`, `build`, `help`, `version`.
- Examples, fixture-based test suites, and CI for formatting, linting and tests on Linux, macOS and Windows.

### Fixed

- Internal compiler error (invalid IR) when `&&` or `||` appeared in an operand or argument after another value, as in `to_string(1) + to_string(a && b)` or `f(x, a || b)`.

### Changed

- `brim-codegen` now depends on `brim-diagnostics` and `brim-runtime`; `BuildOptions` carries the program's source.
- `brim_runtime::MAX_CALL_DEPTH` is the shared call-depth limit of the interpreter and native executables.
- `if`, `else`, `while`, `break`, `continue` and `mut` are now keywords instead of reserved words.
- `E0104` now means "invalid assignment target"; assignment to variables is part of the language.
- The nesting limit (`E0105`) now also applies to blocks and `else if` chains.
