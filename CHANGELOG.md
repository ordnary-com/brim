# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/) once it reaches 1.0.

## [Unreleased]

### Added

- `Set[T]`: `Set()`, `Set(list)`, `add`, `remove`, `contains`, `len`, `to_list`, `union`, `intersection`, `difference`, `is_subset`, `for x in set`, `==` and `print` (`{1, 2}`). The built-in trait `Key` lets generic code use maps and sets; map keys can now be any integer type.
- Math: `abs`, `sqrt`, `floor`, `ceil`, `round` and `pow` (overflow and `abs(i64::MIN)` are `R0001`), and in the prelude `min`, `max`, `clamp`, `PI` and `E`. Native programs now link with libm.
- List methods `map`, `filter`, `any`, `all`, `fold`, `find`, `each`, `min` and `max`, also in the prelude. A line that starts with `.` continues the expression above it, for method chains.
- List methods `sorted()`, `sorted_by(less)`, `reversed()`, `slice(start, end)` and `index_of(x)`. They are written in Brim, in the new module `std.prelude` that every program has. The built-in trait `Ord` allows `<` in generic code.

## [0.3.0] - 2026-10-15

### Added

- Floating-point numbers: the `f64` type, literals (`1.5`, `6.02e23`), arithmetic and comparisons following IEEE 754, `to_float`, `to_int` (run-time error `R0010` for NaN and out-of-range numbers), `to_string`, `print` and `parse_float`. Numbers are written with the fewest digits that read back exactly, the same in the interpreter and native code (checked on 200,000 values). `std.json` gets `as_float` and `from_float`; `brim gen ts` maps `f64` to `number`; every editor grammar knows `f64`. `i64` and `f64` never mix implicitly, and the diagnostics say how to convert. `E0007` now means a floating-point literal too large for `f64`.
- Visibility per field: `pub name: type`; other fields are private to the struct's module (`E0241` when read, assigned or given elsewhere). `pub import module.{X}` re-exports items, and import lists can name constants.
- Tuples: `(i64, string)` types, `(a, b)` values, fields `t.0`, `(p, q)` patterns in `match`, and `let (q, r) = divmod(17, 5)` (`E0251` when the pattern can fail). `==` and `print` work on them, and `brim gen ts` writes TypeScript tuples.
- f-strings: `f"Hello, {name}! You are {age + 1}."` shows each value as `print` does; `{{` and `}}` are literal braces. Plain strings are unchanged. The formatter, tree-sitter grammar and VS Code grammar know them.
- `if` and `match` as values: `let sign = if n < 0 { -1 } else { 1 }`, `return match shape { Circle(r) => r * r, ... }`. A block's value is its last line; branches that return or break need none. `E0250` for an `if` without `else` and for blocks without a value.
- `const NAME: T = value` at the top level (`pub const` usable as `module.NAME`), computed from literals, operators and other constants (`E0249` otherwise), and `loop { ... }`. `const` and `loop` are keywords now.
- `let` without a type: `let total = 0`, `let name = user.name` take the type of the value. `[]`, `[:]` and `None` still need an annotation.
- Traits: `trait Shape { fn area(self) -> f64 }`, `impl Shape for Circle { ... }` (checked against the trait, with `Self` in signatures), and bounds on type parameters (`fn total[T: Shape + Show](...)`) checked at every call. Calls through a bound compile to direct calls of the concrete method. The built-in `Eq` and `Show` allow `==` and `print` in generic code. New errors `E0246`, `E0247` and `E0248`; `trait` is now a keyword. The tree-sitter grammar and the Helix, Zed and Neovim queries know the syntax.
- Functions as values: function types `fn(A, B) -> R`, `fn` expressions (`fn(x: i64) -> i64 { return x + n }`) that copy the variables they use, named functions used as values, and calls of function values (`f(3)`, `button.on_click(3)`, `make_adder(1)(2)`). A closure is a function and a reference-counted environment, the same in the interpreter and in native code. Changing a captured variable is `E0218`. The tree-sitter grammar and the Helix, Zed and Neovim queries know the new syntax.
- More integer types: `i32`, `u8` and `u64`. Literals take the type the context needs, arithmetic and comparisons work on two integers of the same type, and results that do not fit stop the program with `R0001` in the interpreter and in native code. `to_i32`, `to_u8`, `to_u64` and `to_int` convert between them with the same check; `print`, `to_string`, `to_float`, `==` and `match` work on all of them. Bytes are a `List[u8]`: `s.bytes()` and `string_from_bytes(b)`. Every editor grammar knows the new names.
- `Box[T]`: one immutable value on the heap, so a struct or enum can contain itself (`enum Expr { Num(i64), Add(Box[Expr], Box[Expr]) }`). `Box(value)` creates one, `b.value()` reads it. `E0221` now suggests a `Box`. Every editor grammar knows `Box`; `brim gen ts` maps `Box[T]` to `T`.
- `==`, `!=`, `print` and `to_string` work on structs, enums, lists and maps (including `Option` and `Result`) whose values support them. Equality is structural (maps compare in any order); the text form looks like the code that builds the value, with strings quoted: `Point { x: 1, y: 2 }`, `Some("ada")`, `[1, 2]`, `["ada": 36]`. The compiler generates one function per type, in the interpreter and in native code alike. `o == None` no longer needs an annotation.
- `?` converts errors: when the function's error type is an enum with exactly one variant holding the operand's error type, the error is wrapped in that variant (`ParseError` becomes `ConfigError.Parse(...)`). Without such a variant, or with two, `E0233` says what to add or that the choice is ambiguous.
- Richer `match` patterns: patterns nest (`Some(Shape.Circle(r))`, `Ok(0)`), `|` joins alternatives (`1 | 2 | 3`, `Some(Empty) | None`), a lowercase name binds the whole value, and `pattern if condition` adds a guard. Exhaustiveness is checked at every depth, and the error lists example values that no arm matches (`not covered: Some(Rect(_, _))`). Unreachable arms and alternatives warn with `W0004`; alternatives that bind different names are `E0245`. The tree-sitter grammar and the Helix, Zed and Neovim queries know the new syntax.
- A Dockerfile for the notes example (`examples/http`), which CI builds, runs and calls. The example reads `HOST` as well as `PORT`.

### Changed

- Struct fields are private to their module unless declared `pub`. The standard library marks the fields programs use (`http.Request`, `http.Response`, `time.Time`, `time.DateTime`) as `pub`; code that reads fields of its own structs from other modules needs `pub` on them.
- `Box` is a built-in type now, so a struct or enum named `Box` must be renamed (`E0219`).

### Fixed

- Freeing deeply nested values (lists of lists, or chains of boxes) no longer overflows the stack, in the interpreter and in native code.

## [0.2.0] - 2026-10-15

### Added

- Standard library modules written in Brim and built into the compiler, imported with `import std.x`.
- `std.net`: TCP listeners and connections (`listen`, `connect`, `accept`, `read_line`, `read_until`, `read_exact`, `read_all`, `write`, `set_timeout`), in the interpreter and native executables alike.
- `std.http`: an HTTP/1.1 server (`serve`, `Server.next`, `Request.respond`, text, JSON and HTML responses, query parameters, chunked request bodies, 400 for malformed requests) and client (`get`, `post`, `request`, `content-length`, chunked and until-close bodies), plus URL `encode` and `decode`.
- `std.time`: the current time, dates in UTC (`iso`, `date`, `http_date`, `parts`), a monotonic clock for durations, and `sleep`. `std.log`: `info`, `warn` and `error` lines with a timestamp on standard error. The notes example logs every request with its duration.
- Limits in the `std.http` server: request heads over 64 KiB get 431 and bodies over 10 MiB (`Server.with_max_body`) get 413, also for chunked bodies. `net.Conn.read_until_max` reads up to a delimiter with a size limit.
- `https://` requests in `std.http`, through libcurl: the interpreter uses the `curl` crate, and native programs that use the HTTP client link with libcurl (servers and other programs do not). Certificates are checked; errors are `certificate not trusted`, `TLS handshake failed` and the network reasons.
- `std.json`: a `Json` value type, `parse` (RFC 8259, with positions in errors), `stringify`, `pretty`, and functions to build and read values. Numbers keep their text, so no precision is lost.
- `examples/http/notes.brim`, a JSON API for notes built with `std.http` and `std.json`.
- Structs and enums can contain themselves through `List` and `Map`, for trees and JSON-like values (`enum Json { Array(List[Json]), ... }`). The checker already allowed it; lowering looped forever.

### Changed

- Only functions reachable from `main` and the tests are compiled, so unused functions of imported modules cost nothing.

### Fixed

- An assignment to a map entry whose value uses `?` (`m[key] = parse(x)?`) produced invalid IR when the key was not an integer.

## [0.1.1] - 2026-10-14

### Added

- `install.sh` (macOS, Linux) and `install.ps1` (Windows) install the latest release, after checking its checksum.
- A standard library reference on the documentation site, with an example for every part.
- Releases include the VS Code extension as a `.vsix` file, and can publish it to the Visual Studio Marketplace and Open VSX and update a Homebrew formula (`scripts/homebrew-formula.sh`).
- Helix indent and text object queries.
- The editor packages are tested in Neovim, Helix, Kakoune, nano, micro and Kate in CI (`editors/test/run.sh`).

### Fixed

- `?` in a method with `mut self` produced invalid IR (an internal compiler error): its early return now also gives back `self`.

## [0.1.0] - 2026-10-14

### Added

- Enums: `enum Shape { Circle(i64), Rect(i64, i64), Empty }`, constructed with `Shape.Circle(1)`, and `match` statements with variant, literal and `_` patterns, exhaustiveness checking and unreachable-arm warnings (`W0004`).
- Generics: type parameters on functions, structs and enums (`fn swap[A, B](p: Pair[A, B]) -> Pair[B, A]`), with type arguments inferred from arguments and annotations, compiled by monomorphization.
- Built-in `Option[T]` (`Some`, `None`) and `Result[T, E]` (`Ok`, `Err`).
- The `?` operator: `parse(s)?` continues with the `Ok`/`Some` value, or returns the `Err`/`None` from the function (`E0232`, `E0233`).
- New diagnostics `E0225` to `E0231` and `W0004`; `enum` and `match` are now keywords, and `[`, `]` and `=>` are tokens.
- `examples/shapes.brim`.
- `List[T]`: literals (`[1, 2, 3]`), indexing (`xs[i]`, run-time error `R0005` outside the list), element assignment (`xs[i] = v`), and the methods `len`, `is_empty`, `get`, `first`, `last`, `contains`, `join`, `push`, `pop`, `insert`, `remove` and `clear`. Lists are values with copy-on-write; changing a list that is not shared happens in place, also inside structs and nested lists.
- `for x in list { ... }` and `for i in a..b { ... }`; `for` and `in` are keywords and `..` is a token.
- IR: list types and instructions, `take`, and consumed operands (checked by the validator).
- New diagnostics `E0234` and `E0235`; `examples/lists.brim`.
- `Map[K, V]` with `i64`, `bool` or `string` keys: literals (`["a": 1]`, `[:]`), indexing (`m[k]`, run-time error `R0006` for a missing key), assignment through keys, and the methods `len`, `is_empty`, `get`, `contains_key`, `keys`, `values`, `insert`, `remove` and `clear`. Entries keep insertion order. Maps are values with copy-on-write.
- `E0236` for map keys of other types; `examples/maps.brim`.
- Modules: every file is a module; `import a.b`, `import a.b as c` and `import a.b.{X, y}`, with `pub` for items and methods. Diagnostics show labels in other files with `:::`. New diagnostics `E0241` to `E0243`; `import`, `pub` and `as` are keywords. `tests/modules` holds multi-module test programs.
- Standard library: string methods (`len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `find`, `slice`, `trim`, `trim_start`, `trim_end`, `to_upper`, `to_lower`, `replace`, `repeat`, `split`, `lines`, `chars`, `parse_int`), string comparison with `<` and friends, and the builtins `eprint`, `read_line`, `read_file`, `write_file`, `args`, `env` and `exit`. Run-time error `R0007` for invalid string slices.
- `brim test [PATH] [FILTER]` runs the program's tests, the functions named `test_...`, and reports each one; `main` is optional for it. The builtins `assert` and `assert_eq` (on `i64`, `bool` and `string`) stop the program with run-time error `R0008`, which shows both values. New diagnostic `E0244`.
- A documentation site in `site/` (mdBook): introduction, getting started, a tour of the language, projects and tools, and the reference documents from `docs/`. A test runs every example on it and compares the output; a workflow builds it and can publish it to GitHub Pages.
- A release process (`docs/releasing.md`) with stability rules per edition, and a release workflow that builds `brim` for Linux (x86_64, aarch64), macOS (aarch64) and Windows (x86_64), plus Linux and macOS builds with the LLVM backend linked in statically (new feature `llvm-static`). `brim --version` says whether the native backend is included.
- Fuzzing: `crates/brim-cli/tests/fuzz_smoke.rs` runs mutants of all example and test programs through the frontend, the formatter and the interpreter with every test run; `fuzz/` has cargo-fuzz targets (`frontend`, `fmt`, `run`, `manifest`), run weekly in CI.
- `brim_interp::Limits::max_steps`, an optional limit on executed instructions for embedders, with run-time error `R0009`.
- `brim lsp`, a language server over standard input and output (new crate `brim-lsp`): diagnostics for every file of the program as you type, formatting, document symbols, go to definition and hover for variables, parameters, functions, types, fields, variants, methods and modules. Program loading moved to the new crate `brim-driver`, shared with the command line.
- Support for every common editor in `editors/`: a tree-sitter grammar (`tree-sitter-brim`, parsing every example and test program) with packages for Neovim, Helix and Zed, and highlighting plus language server setup for Emacs (`brim-mode`), Sublime Text (also bat), JetBrains IDEs, Kate, Kakoune, Vim, nano, micro and Notepad++. CI checks the grammars and that every one knows all keywords and builtins.
- Editor support in `editors/`: a VS Code extension (TextMate grammar, file icon for `.brim`, language configuration, language client) and a Vim syntax file. A test keeps the grammars in step with the keywords and builtins.
- `brim gen ts [PATH] [-o FILE]` writes TypeScript declarations for the program's `pub` structs and enums and the types they use, describing them as JSON (see the language specification, section 9.4). New crate `brim-tsgen`, error `G0001`, `examples/api.brim` with `examples/api.ts`.
- Dependencies: `[dependencies]` in `brim.toml` with `{ path = "..." }` or `{ git = "...", tag/branch/rev = "..." }`. `import dep` uses the dependency's library root (`src/lib.brim`, or `[build] lib`), `import dep.a.b` its other modules. Git dependencies are cached in `$BRIM_HOME/git` and their commits recorded in `brim.lock`; `brim update` moves them to the newest commits. New crate `brim-package`; manifest errors `M0006` to `M0010`.
- `brim fmt [--check] [PATH...]` formats source files in place (or standard input with `-`): indentation, spaces between tokens and blank lines, keeping comments and line breaks. It refuses files with syntax errors and verifies that formatting does not change the tokens. New crate `brim-fmt`.
- `brim new PATH` creates a project with a manifest, `src/main.brim` with a test, and a `.gitignore`.
- `brim run PROGRAM ARGS...` passes arguments to the program, and the program's exit code becomes `brim run`'s.
- `examples/wc.brim`, a small `wc`.
- Methods: `impl Type { fn name(self, ...) }`, with `self` (a copy), `mut self` (changes the caller's value, which is moved into the method so lists in it change in place) and methods without `self` called on the type (`Stack.new()`). Generic types get generic `impl` blocks. New diagnostics `E0237` to `E0240`; `impl` and `self` are keywords.
- Structs: `struct` declarations, struct literals (`Point { x: 1, y: 2 }`), field access (`p.x`) and field assignment (`p.x = 3`, `p.from.y += 1`), with value semantics. Supported by the interpreter and the native backend, including reference counting of strings inside structs. New diagnostics `E0219` to `E0224`; `struct` is now a keyword.
- `E0007` for floating-point literals such as `9.99`.
- `examples/structs.brim`.
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
