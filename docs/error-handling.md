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
| E0101 | Reserved keyword used (`for`, `trait`, `impl`, ...): the feature is not supported yet |
| E0102 | Missing type annotation on a `let`, a parameter or a struct field |
| E0103 | Something other than a function, struct or enum declaration at the top level, or a struct or enum declared inside a function |
| E0104 | Invalid assignment target, or a list method that changes the list called on something other than a variable (only variables, their fields and list elements can be changed) |
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
| E0214 | Function, struct or enum used as a value, or a variable called as a function |
| E0215 | Integer literal out of range for `i64` |
| E0216 | Function redefines a builtin |
| E0217 | `break` or `continue` outside of a loop |
| E0218 | Assignment to an immutable variable, a parameter, or a field of one |
| E0219 | Type name already used: by another struct or enum, a built-in type (including `Option` and `Result`) or a function; or a type parameter named like a built-in type |
| E0220 | Field, variant or type parameter declared more than once |
| E0221 | Struct or enum contains itself, directly or through other types |
| E0222 | Struct literal is missing fields |
| E0223 | Field given more than once in a struct literal |
| E0224 | Unknown field, or field access on a value that is not a struct |
| E0225 | Wrong number of type arguments for a generic type (or in an `impl`), or type arguments on a type that takes none |
| E0226 | A type argument of a generic function, struct or variant cannot be inferred |
| E0227 | Unknown enum variant |
| E0228 | Wrong number of values for an enum variant, in a constructor or a pattern |
| E0229 | `match` does not cover every value |
| E0230 | Pattern does not fit the type of the matched value |
| E0231 | Unknown method |
| E0232 | `?` applied to a value that is not an `Option` or `Result` |
| E0233 | `?` in a function whose return type cannot carry the `Err` or `None` (a different error type, or not a `Result`/`Option`) |
| E0234 | Indexing a value that is not a list or map |
| E0235 | `for` loop over a value that is not a list or range |
| E0236 | Map key type other than `i64`, `bool` or `string` |
| E0237 | `impl` block for a built-in type |
| E0238 | Method defined twice for a type, or named like one of the enum's variants |
| E0239 | `self` parameter outside an `impl` block |
| E0240 | Method with `self` called on the type, or method without `self` called on a value |

### Warnings

| Code  | Meaning |
|-------|---------|
| W0001 | Unreachable code after `return`, `break`, `continue` or a diverging `if`/`while` |
| W0002 | Value of a non-call expression statement is unused |
| W0003 | Variable declared `mut` but never reassigned |
| W0004 | Unreachable `match` arm: earlier arms already cover it |

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
| R0005 | List index out of bounds |
| R0006 | Map key not found |

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

## Errors inside Brim programs

Brim has two kinds of failures:

- **Recoverable errors are values.** A function that can fail returns `Result[T, E]`: `Ok(value)` or `Err(error)`. The compiler makes callers deal with it: a `Result` is not a `T`, so the value can only be used after a `match` or the `?` operator, which returns the `Err` from the calling function unchanged:

  ```brim
  fn read_config(path: string) -> Result[Config, string] {
      let text: string = read_file(path)?
      return parse_config(text)
  }
  ```

  Absent values use `Option[T]` the same way; there is no null.
- **Run-time errors abort.** The `R` errors above (overflow, division by zero, stack overflow) are bugs, not conditions to handle: output written so far is flushed, the error is reported on stderr, and the process exits with 101.

Not designed yet: converting one error type into another in `?` (today the types must be equal), a standard error type with context, and returning a `Result` from `main`.

Brim has no exceptions with stack unwinding: they complicate the native runtime and make control flow invisible.
