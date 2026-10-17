<p align="center">
  <img src="assets/brim-logo.png" alt="Brim" width="320">
</p>

<p align="center">
  <strong>A statically typed, compiled language for backend services, by Ordnary.</strong>
</p>

<p align="center">
  <a href="#why-brim">Why Brim</a> ·
  <a href="#status">Status</a> ·
  <a href="#getting-started">Getting started</a> ·
  <a href="docs/language-spec.md">Language spec</a> ·
  <a href="docs/roadmap.md">Roadmap</a>
</p>

---

Brim is a programming language for the kind of software Ordnary builds every day: APIs, backend services, CLI tools and developer tooling. It compiles ahead of time, keeps Go's simplicity, and fixes the problems we keep running into in Go.

```brim
fn fizzbuzz(n: i64) -> string {
    if n % 15 == 0 {
        return "FizzBuzz"
    } else if n % 3 == 0 {
        return "Fizz"
    } else if n % 5 == 0 {
        return "Buzz"
    }
    return to_string(n)
}

fn main() {
    let mut i: i64 = 1
    while i <= 15 {
        print(fizzbuzz(i))
        i += 1
    }
}
```

> **Brim is heading for 1.0.** Programs run on a reference interpreter or compile to native executables with LLVM, on Linux, macOS and Windows. Go is still Ordnary's production backend. See [Status](#status) for what 1.0 covers.

## Why Brim

Ordnary's backend is written in Go, and our frontend in Angular. Go served us well: it compiles fast, deploys as a single binary, and is easy to read. But as our services grew, the same classes of bugs and friction kept coming back, and most of them are not fixable in Go itself because they come from its core design decisions.

Brim is our answer: keep what makes Go productive, and design out the parts that cost us time.

### What keeps going wrong in Go

**1. The backend and the frontend do not share a type system.**
Every API type exists twice: as a Go struct with `json:"..."` tags, and as a hand-written TypeScript interface in Angular. Rename a field in Go and the Angular app still compiles; it only breaks at run time, in a browser. In Brim, API types are declared once: `brim gen ts --angular` generates the TypeScript types and an Angular client from them, so a breaking change becomes a compile error on both sides.

**2. Errors are easy to ignore.**
`if err != nil { return err }` appears everywhere, and `result, _ := doSomething()` silently throws an error away. Nothing forces the caller to look at it. In Brim, errors are `Result` values the compiler makes you handle, and `?` passes them up.

**3. `nil` is everywhere.**
Nil pointer dereferences, writes to nil maps, and the "typed nil inside an interface is not equal to nil" trap are all run-time panics in Go. Brim has no null value at all: absence is an `Option`, which the compiler checks.

**4. No real enums.**
Go has `iota` constants, not enums. A `switch` over them is never checked for missing cases, and any integer can sneak in as a "status". Brim has enums (sum types) with exhaustive `match`: add a new case and the compiler lists every place that needs to handle it.

**5. Integer overflow is silent.**
In Go, `math.MaxInt64 + 1` quietly wraps around to a large negative number. Brim checks every integer operation and stops with an error that points at the exact expression:

```text
error[R0001]: integer overflow in `+`
 --> billing.brim:3:12
  |
3 |     return total + fee
  |            ^^^^^^^^^^^
  |
  = note: call stack: main -> charge
```

**6. Everything is mutable, and zero values hide mistakes.**
In Go any variable can be reassigned from anywhere in its scope, and forgetting to set a field or a variable silently gives you `0`, `""` or `nil`. In Brim, variables are immutable unless declared `let mut`, and every variable must be initialised explicitly; there are no implicit zero values.

**7. The compiler blocks you for the wrong things and helps too little with the right ones.**
Go refuses to build while you have an unused variable or import, which interrupts every debugging session, but gives terse messages for real type errors. Brim reports harmless leftovers (an unneeded `mut`, unreachable code) as warnings, and real errors with codes, source excerpts and suggestions:

```text
error[E0218]: cannot assign to immutable variable `count`
 --> handler.brim:8:5
  |
7 |     let count: i64 = 0
  |         ----- declared here
8 |     count = count + 1
  |     ^^^^^ cannot be reassigned
  |
  = help: make it mutable: `let mut count`
```

### Go vs Brim at a glance

| | Go | Brim | Brim status |
|---|---|---|---|
| Integer overflow | wraps silently | stops with a located error | **available** |
| Variables | mutable by default | immutable by default, `let mut` to opt in | **available** |
| Uninitialised values | implicit zero values | every variable is initialised explicitly | **available** |
| Null | `nil` pointers, maps, interfaces | no null value; `Option[T]` | **available** |
| Unused code | build error | warning | **available** |
| Compiler errors | short messages | error codes, excerpts, suggestions | **available** |
| Enums | `iota` constants | sum types with exhaustive `match` | **available** |
| Error handling | `if err != nil`, easy to ignore | `Result` values and `?`, enforced by the compiler | **available** |
| API types shared with Angular | written twice, by hand | declared once, TypeScript and an Angular client generated | **available** |
| Concurrency | goroutines with shared memory, data races possible | tasks and channels, no shared changeable memory | **available** |
| Native single-binary deploys | yes | yes | **available** (LLVM; Linux, macOS, Windows) |

### What we keep from Go

Fast builds, a small language you can learn in a day, readable code without clever tricks, a single binary to deploy, and tooling in the box. Brim is not trying to be Rust: there is no borrow checker, and memory will be managed automatically (see [Memory management](docs/architecture.md#memory-management)).

## Status

Brim is heading for **1.0**: the language and its standard library are complete for backend services and command-line tools, and from 1.0 they are stable (see [Releasing](docs/releasing.md#stability)).

- **Language:** structs, enums with exhaustive `match`, generics, traits, closures, tuples, `Option` and `Result` with `?`, lists, maps and sets with copy-on-write value semantics, `i64`, `i32`, `u8`, `u64`, `f64`, checked arithmetic, f-strings, modules with `pub`. The [specification](docs/language-spec.md) describes all of it.
- **Concurrency:** tasks on threads with channels (`std.task`), without shared changeable memory.
- **Standard library:** files, processes, time, logging, random numbers, TCP, an HTTP/1.1 server and client (HTTPS through the system), JSON with typed `encode`/`decode`, PostgreSQL, hashes and base64.
- **Tools:** one `brim` command to run, check, build, test, format, document (`brim doc`), generate TypeScript and Angular clients (`brim gen ts`), and manage packages (`brim publish`, workspaces, `brim.lock`); a language server and packages for 13 editors (see `editors/`).
- **Backends:** a reference interpreter, and native executables through LLVM 22 on Linux, macOS and Windows, with debug information for lldb and gdb. Both behave the same; the tests compare them on every program.
- **Packages:** a registry at [registry.brimlang.com](https://registry.brimlang.com), and dependencies from directories and git.

Not in 1.0: the items in [section 12 of the specification](docs/language-spec.md#12-not-in-10), TLS for `std.postgres` and HTTP servers (put a proxy in front), and cross-compilation. The [roadmap](docs/roadmap.md) lists what comes next.

## Getting started

Install the latest release on macOS or Linux:

```sh
curl -fsSL https://brimlang.com/install.sh | sh
```

On Windows (PowerShell): `irm https://brimlang.com/install.ps1 | iex`.

To build from source, you need Rust 1.88 or newer, installed with [rustup](https://rustup.rs). LLVM is only needed for native builds (see below).

```sh
git clone https://github.com/ordnary-com/brim.git
cd brim
cargo build --release
```

The compiler is `target/release/brim`, or install it with `cargo install --path crates/brim-cli`.

```sh
brim run examples/hello.brim          # check and run a program
brim run examples/control_flow.brim   # FizzBuzz and primes
brim check examples/variables.brim    # report errors without running
brim check --emit ir examples/functions.brim   # show the intermediate representation
brim run examples/project             # run a project described by brim.toml
brim new hello && cd hello && brim test   # create a project and run its tests
brim fmt                              # format the project's source files
brim --help
```

To compile native executables, build `brim` with the `llvm` feature. It needs LLVM 22 and a C compiler; the [development guide](docs/development.md#native-builds-llvm) has the setup.

```sh
export LLVM_SYS_221_PREFIX="$(brew --prefix llvm@22)"   # macOS; see the guide for Linux
cargo build --release --features brim-cli/llvm
brim build examples/control_flow.brim && ./examples/control_flow
brim build --release examples/project   # writes examples/project/target/hello
```

A project is a directory with a `brim.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"
edition = "2026"

[build]
entry = "src/main.brim"
```

## Contributing

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) and the [development guide](docs/development.md) first. Language changes start with an update to the [language specification](docs/language-spec.md).

<details>
<summary>Repository layout</summary>

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
  brim-codegen      LLVM backend and native runtime (`llvm` feature)
  brim-runtime      runtime ABI and runtime functions
  brim-stdlib       builtin function catalogue
  brim-manifest     brim.toml parsing and validation
  brim-fmt          the formatter
  brim-package      dependencies and brim.lock
  brim-tsgen        TypeScript types for API contracts
  brim-driver       loads a program's modules (shared by the command and the language server)
  brim-lsp          the language server
  brim-cli          the `brim` command
assets/             logo and other project artwork
docs/               language spec, architecture, error handling, roadmap, development
editors/            support for VS Code, Neovim, Helix, Zed, Emacs, JetBrains, Sublime and more
site/               documentation site (mdBook)
examples/           example programs and an example project
tests/              lexer, parser, diagnostics and integration fixtures
```

</details>

## Documentation

- [Documentation site](https://brimlang.com/): a tour of the language, guides and the reference (source in `site/`)
- [Language specification](docs/language-spec.md)
- [Architecture](docs/architecture.md)
- [Error handling and diagnostic codes](docs/error-handling.md)
- [Roadmap](docs/roadmap.md)
- [Development guide](docs/development.md)
- [Releasing and stability](docs/releasing.md)
- [Security policy](SECURITY.md) · [Changelog](CHANGELOG.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
