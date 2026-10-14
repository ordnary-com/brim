# Development Guide

## Prerequisites

- A Rust toolchain installed with [rustup](https://rustup.rs). The repository's `rust-toolchain.toml` selects the stable channel with `rustfmt` and `clippy`. The minimum supported Rust version is 1.88 (let-chains).
- No LLVM installation is needed for the compiler frontend, `brim check` and `brim run`.
- For native builds (`brim build`) and their tests: LLVM 22 and a C compiler. See [Native builds](#native-builds-llvm).

> **macOS / Homebrew note:** if `rustc` was also installed through Homebrew (`brew install rust`), the Homebrew binaries may come first on `PATH` and ignore `rust-toolchain.toml`. Homebrew's `rustc` can also break when Homebrew upgrades LLVM (`dyld: Symbol not found ... libLLVM`). Prefer rustup's toolchain: put rustup's proxy directory first on your `PATH`. That is `~/.cargo/bin` for a standard rustup install, or `$(brew --prefix rustup)/bin` for Homebrew's `rustup` formula. Alternatively, run `rustup run stable cargo ...`.

## Everyday commands

```sh
cargo build                                   # build everything
cargo test --workspace                        # unit, fixture and CLI tests
cargo fmt --all                               # format
cargo clippy --workspace --all-targets -- -D warnings

cargo run -q -p brim-cli -- run examples/hello.brim
cargo run -q -p brim-cli -- check --emit ir examples/functions.brim
```

The binary is written to `target/debug/brim`.

## Native builds (LLVM)

The LLVM backend is behind the `llvm` Cargo feature of `brim-cli` (and `brim-codegen`). It needs LLVM **22** (not 23) and a C compiler for linking (`cc`, or set `BRIM_CC`). Tell `llvm-sys` where LLVM is with `LLVM_SYS_221_PREFIX`:

```sh
# macOS
brew install llvm@22
export LLVM_SYS_221_PREFIX="$(brew --prefix llvm@22)"

# Debian / Ubuntu (apt.llvm.org)
wget -qO- https://apt.llvm.org/llvm.sh | sudo bash -s -- 22
sudo apt-get install -y libpolly-22-dev libzstd-dev
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
```

Then:

```sh
cargo build --features brim-cli/llvm
cargo test --workspace --features brim-cli/llvm   # includes the differential tests
cargo clippy --workspace --all-targets --features brim-cli/llvm -- -D warnings

target/debug/brim build examples/hello.brim       # writes examples/hello
target/debug/brim build --release -o /tmp/app examples/control_flow.brim
```

Without `LLVM_SYS_221_PREFIX`, `llvm-sys` looks for `llvm-config` on `PATH`, which is often a different LLVM version.

To debug memory management in a native executable, run it with `BRIM_RT_CHECK=1`: use of a freed string aborts, and leaked strings are reported when the program ends. The differential tests always set it. When changing how the backend handles strings, also check that the tests still fail when a retain or release is removed.

## Test layout

| Location | What it covers |
|----------|----------------|
| `crates/*/src` (`#[cfg(test)]`) | Unit tests for each crate: lexer tokens and spans, parser precedence and recovery, type checking, IR validation, interpreter semantics, manifest validation, argument parsing |
| `tests/lexer`, `tests/parser`, `tests/diagnostics` | Programs that must fail `brim check`, with the exact expected stderr in a `.stderr` file next to each program |
| `tests/integration` | Programs run with `brim run`. Expected stdout in `.stdout`, expected stderr (if any) in `.stderr` |
| `crates/brim-cli/tests/fixtures.rs` | The harness for the four directories above, plus a check that every program in `examples/` runs. With the `llvm` feature it also builds every integration program and example natively (debug and release) and requires output identical to the interpreter's |
| `crates/brim-cli/tests/cli.rs` | Runs the real `brim` executable: exit codes, help, manifests |

Fixture conventions:

- Programs in the `check` suites are expected to exit with 1; integration programs with 0.
- A first line `// exit: N` overrides the expected exit code.
- A missing `.stdout` or `.stderr` file means that stream must be empty.

### Updating expected output

After an intentional change to diagnostics or program output:

```sh
BLESS=1 cargo test -p brim-cli --test fixtures
git diff tests/          # review every change
```

## Adding a language feature

1. Specify it in `docs/language-spec.md`, including grammar, typing rules and new diagnostics. Move it out of "open questions" or "future features" if applicable.
2. Lexer: new tokens or keywords. If the keyword was reserved, move it from `RESERVED` to `KEYWORDS`.
3. AST and parser: new nodes, parse functions and tests (valid and invalid input).
4. Semantic analysis: checking rules, new error codes with tests, and the checked-tree representation.
5. IR: new instructions if needed, plus validator rules and tests.
6. Lowering, then the interpreter, then the LLVM backend (`brim-codegen/src/llvm.rs`, and `runtime/brim_rt.c` for new runtime functions). Run the tests with `--features brim-cli/llvm` so the differential tests compare both.
7. Fixtures in `tests/`, an entry in `docs/error-handling.md` for each new code, and an entry in `CHANGELOG.md`.

## Adding a diagnostic

- Pick the next free code in the right range (`E00xx` lexer, `E01xx` parser, `E02xx` semantic, `Wxxxx` warnings, `Mxxxx` manifest, `Rxxxx` run time).
- Point the primary span at the cause. Explain with secondary labels and notes, and offer `help` when there is a likely fix.
- Add a fixture that shows the rendered output and document the code in `docs/error-handling.md`.

## Coding conventions

- Stable Rust only. `unsafe` code is forbidden workspace-wide (`unsafe_code = "forbid"`); lifting that requires a documented justification.
- The compiler must not panic on any input. Use diagnostics. `expect`/`unreachable!` are reserved for invariants guaranteed by earlier phases, with a message saying which phase.
- Keep crates independent: the IR must not depend on the AST, and nothing in the frontend may depend on LLVM.
- New third-party dependencies need a clear justification and an exact version pin in the workspace `Cargo.toml`.
