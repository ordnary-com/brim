# Error Handling

This document covers three things: how the compiler reports problems (diagnostics), how the `brim` tool signals failure (exit codes), and how errors work inside Brim programs (run-time errors, plus the open design for recoverable errors).

## Diagnostics

Every problem the toolchain finds is a `Diagnostic` (`crates/brim-diagnostics`) with:

- a **severity**: `error` (the program is rejected) or `warning` (accepted);
- an optional stable **code** such as `E0200`;
- a **message**;
- an optional **primary label**: a span marked with `^`, plus a short label;
- any number of **secondary labels**, marked with `-`, for related locations;
- **notes** (`= note:`) for context and **help** (`= help:`) for suggested fixes.

Rendered output:

```text
error[E0206]: mismatched types
 --> tests/diagnostics/type_mismatch.brim:6:25
  |
6 |     let label: string = 42
  |                ------ expected due to this type
  |                         ^^ expected `string`, found `i64`
  |
  = note: Brim never converts between types implicitly
```

Colors are used when stderr is a terminal and `NO_COLOR` is unset or empty.

Guidelines for writing diagnostics:

- State the problem in the message and point at the exact cause with the primary span. Use secondary labels to explain *why* something was expected.
- Suggest a concrete fix in `help` when there is a likely one, such as a "did you mean" for typos or the correct syntax.
- Never let one mistake produce a cascade. The parser recovers at statement boundaries and skips tokens the lexer already reported. The type checker uses an internal error type that suppresses follow-up errors.
- Every new code gets a fixture in `tests/` and an entry below. Codes are never reused for a different problem.

## Diagnostic codes

### Lexical errors (`brim-lexer`)

| Code  | Meaning |
|-------|---------|
| E0001 | Unexpected character (including single `&` or `|`, `'`, non-ASCII outside strings) |
| E0002 | Unterminated string literal (strings must end on the same line) |
| E0003 | Invalid escape sequence in a string |
| E0004 | Integer literal does not fit in 64 bits |
| E0005 | Integer literal with leading zeros |
| E0006 | Invalid suffix on an integer literal (`10px`, `0xFF`) |
| E0007 | Floating-point literal (`9.99`); not supported yet |

### Syntax errors (`brim-parser`)

| Code  | Meaning |
|-------|---------|
| E0100 | Unexpected token ("expected X, found Y"), including unclosed blocks and parentheses |
| E0101 | Reserved keyword used (`for`, `enum`, `match`, ...): the feature is not supported yet |
| E0102 | Missing type annotation on a `let`, a parameter or a struct field |
| E0103 | Something other than a function or struct declaration at the top level, or a struct declared inside a function |
| E0104 | Invalid assignment target (only variables and their fields can be assigned to) |
| E0105 | Blocks or expressions nested too deeply (implementation limit: 128) |
| E0106 | Chained comparison such as `a < b < c` |
| E0107 | Call of something that is not a function name (including method-style calls `p.f()`) |

### Semantic errors (`brim-semantic`)

| Code  | Meaning |
|-------|---------|
| E0200 | Undefined variable |
| E0201 | Undefined function |
| E0202 | Function defined more than once |
| E0203 | Variable declared twice in the same scope |
| E0204 | Duplicate parameter name |
| E0205 | Wrong number of arguments |
| E0206 | Mismatched types, including a non-`bool` condition and a builtin called with an unsupported argument type |
| E0207 | Non-`void` function can reach its end without returning |
| E0208 | `return` without a value in a non-`void` function, or with a value in a `void` function |
| E0209 | Operator applied to unsupported operand types |
| E0210 | No `main` function |
| E0211 | `main` has parameters or a return type |
| E0212 | Unknown type name, including an unknown struct in a struct literal |
| E0213 | `void` used as a variable, parameter or field type |
| E0214 | Function or struct used as a value, or a variable called as a function |
| E0215 | Integer literal out of range for `i64` |
| E0216 | Function redefines a builtin |
| E0217 | `break` or `continue` outside of a loop |
| E0218 | Assignment to an immutable variable, a parameter, or a field of one |
| E0219 | Struct name already used: by another struct, a built-in type or a function |
| E0220 | Field declared more than once in a struct |
| E0221 | Struct contains itself, directly or through other structs |
| E0222 | Struct literal is missing fields |
| E0223 | Field given more than once in a struct literal |
| E0224 | Unknown field, or field access on a value that is not a struct |

### Warnings

| Code  | Meaning |
|-------|---------|
| W0001 | Unreachable code after `return`, `break`, `continue` or a diverging `if`/`while` |
| W0002 | Value of a non-call expression statement is unused |
| W0003 | Variable declared `mut` but never reassigned |

### Manifest errors (`brim-manifest`)

| Code  | Meaning |
|-------|---------|
| M0001 | Invalid TOML, a missing required field, or an unknown key |
| M0002 | Invalid package name |
| M0003 | Invalid package version |
| M0004 | Unsupported edition |
| M0005 | Invalid `[build] entry` path |
| M0006 | `[dependencies]` used; not supported yet |

### Run-time errors (`brim-runtime`)

| Code  | Meaning |
|-------|---------|
| R0001 | Integer overflow |
| R0002 | Division or remainder by zero |
| R0003 | Call depth limit exceeded (stack overflow) |
| R0004 | Writing program output failed |

Run-time errors are reported like compile-time diagnostics, with the source location of the failing operation and the call stack:

```text
error[R0001]: integer overflow in `*`
 --> tests/integration/overflow.brim:3:12
  |
3 |     return n * 2
  |            ^^^^^
  |
  = note: call stack: main -> double
```

## Exit codes of `brim`

| Code | Meaning |
|------|---------|
| 0    | Success (warnings may have been printed) |
| 1    | The program has syntax or semantic errors |
| 2    | Invalid command line (unknown command or option, missing input) |
| 3    | Input problem: file missing or unreadable, not UTF-8, wrong extension, invalid `brim.toml` |
| 4    | Requested feature unavailable: `brim build` in a `brim` compiled without the `llvm` feature, or no C compiler to link with |
| 70   | Internal compiler error (a bug in Brim) |
| 101  | The Brim program stopped with a run-time error |

## Internal errors

The compiler must never panic on any input. Fuzz-style tests (`garbage_input_does_not_panic`, deep nesting tests) guard the lexer and parser. Invalid IR from lowering is caught by the IR validator and reported as an internal error (exit 70). A panic anywhere is caught at the top of `brim`, reported as an internal compiler error, and exits with 70.

## Errors inside Brim programs: current model and open design

In Brim 0.1 the only run-time failures are the `R` errors above, and they all **abort** the program: output written so far is flushed, the error is reported on stderr, and the process exits with 101. There is no way to catch or recover from them inside a program.

A recoverable error mechanism is required before Brim is useful for servers and CLI tools, for example to report "file not found" without terminating. It is an open design question (see `language-spec.md`, section 11). The leading candidate:

- errors as values, through a built-in result type, once generics or a special form exist;
- explicit propagation with a short operator;
- aborting errors kept for genuine bugs (overflow, failed assertions), as today.

Exceptions with stack unwinding are considered undesirable: they complicate the native runtime and make control flow invisible. No decision has been made, and nothing in the current implementation depends on one.
