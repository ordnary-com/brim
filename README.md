<p align="center">
  <img src="assets/brim-logo.png" alt="Brim" width="320">
</p>

<p align="center">The Brim programming language, by Ordnary</p>

# Brim

Brim is a statically typed, compiled, general-purpose programming language developed by **Ordnary**. It is aimed at backend services, APIs, server software, CLI tools and developer tooling, and eventually broader systems programming. Its intended compilation target is native executables via LLVM.

```brim
fn is_prime(n: i64) -> bool {
    if n < 2 {
        return false
    }
    let mut divisor: i64 = 2
    while divisor * divisor <= n {
        if n % divisor == 0 {
            return false
        }
        divisor += 1
    }
    return true
}

fn main() {
    let mut n: i64 = 1
    while n <= 20 {
        if is_prime(n) {
            print(to_string(n) + " is prime")
        }
        n += 1
    }
}
```

## Status

Brim is at **version 0.1, an early foundation**. It is not ready for real use.

What works today:

- The complete frontend for the 0.1 language: lexer, parser, semantic analysis and type checking, with detailed diagnostics (error codes, source excerpts, suggestions).
- Language features: functions with typed parameters and return types, `main` as entry point, `let` bindings with type annotations (immutable unless `let mut`), assignment and compound assignment, `if`/`else`, `while` with `break`/`continue`, `i64`/`bool`/`string`, arithmetic, comparison and logical operators, string concatenation and equality, nested blocks with shadowing, `//` comments, newline-terminated statements, and the builtins `print` and `to_string`.
- A typed intermediate representation (Brim IR) with a validator.
- **Execution through a reference interpreter** (`brim run`). This is a temporary execution path, not the final backend.
- The `brim.toml` project manifest (package name, version, edition, entry file).

What is **not** implemented yet:

- **Native code generation.** There is no LLVM backend yet; `brim build` reports this and exits with code 4.
- `for` loops and `match`, structs, enums, collections, generics, traits, modules and imports, closures, error handling in the language, async, macros.
- Dependencies, a package registry, a formatter, a language server.

See [`docs/roadmap.md`](docs/roadmap.md) for the plan. The next milestone is native code generation.

## Prerequisites

- Rust stable, version 1.88 or newer, installed with [rustup](https://rustup.rs). The repository's `rust-toolchain.toml` selects the right toolchain automatically.
- No LLVM installation is required.

## Building

```sh
git clone https://github.com/ordnary-com/brim.git
cd brim
cargo build --release
```

The compiler binary is `target/release/brim`. Or install it with `cargo install --path crates/brim-cli`.

## Running Brim programs

```sh
brim run examples/hello.brim         # check and run with the reference interpreter
brim run examples/control_flow.brim  # FizzBuzz and primes
brim check examples/variables.brim   # report errors without running
brim check --emit ir examples/functions.brim   # show the generated Brim IR
brim run examples/project            # run a project described by brim.toml
brim --help
```

During development you can use `cargo run -q -p brim-cli -- run examples/hello.brim` instead.

Example error output:

```text
error[E0200]: undefined variable `nmae`
 --> tests/diagnostics/undefined_variable.brim:5:11
  |
5 |     print(nmae)
  |           ^^^^ not found in this scope
  |
  = help: a variable with a similar name exists: `name`
```

## Running the tests

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The test suite includes unit tests for every crate, snapshot tests over the programs in `tests/` (expected diagnostics and output), and end-to-end tests of the `brim` executable. See [`docs/development.md`](docs/development.md).

## Repository layout

```text
crates/
  brim-diagnostics  source files, spans, diagnostics and their rendering
  brim-lexer        source text to tokens
  brim-ast          syntax tree
  brim-parser       tokens to syntax tree
  brim-semantic     name resolution and type checking
  brim-ir           intermediate representation and validator
  brim-lower        checked program to IR
  brim-interp       reference interpreter (temporary execution path)
  brim-codegen      native backend interface (LLVM planned)
  brim-runtime      runtime ABI and runtime functions
  brim-stdlib       builtin function catalogue
  brim-manifest     brim.toml parsing and validation
  brim-cli          the `brim` command
assets/             logo and other project artwork
docs/               language spec, architecture, error handling, roadmap, development
examples/           example programs and an example project
tests/              lexer, parser, diagnostics and integration fixtures
```

## Documentation

- [Language specification](docs/language-spec.md)
- [Architecture](docs/architecture.md)
- [Error handling and diagnostic codes](docs/error-handling.md)
- [Roadmap](docs/roadmap.md)
- [Development guide](docs/development.md)
- [Contributing](CONTRIBUTING.md), [Security policy](SECURITY.md), [Changelog](CHANGELOG.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
