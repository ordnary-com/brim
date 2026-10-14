//! Runtime support for Brim programs.
//!
//! This crate defines the boundary between compiled Brim code and the
//! functionality it needs from its host environment:
//!
//! - [`abi`] lists every runtime function generated code may call, with its
//!   symbol name and signature. The IR refers to runtime functions only through
//!   [`RuntimeFn`], so the same IR can be executed by the reference interpreter
//!   today and linked against a native runtime library later.
//! - [`ops`] implements the semantics of operations that can fail at run time
//!   (checked integer arithmetic).
//! - [`RuntimeError`] describes run-time failures and their stable codes.
//!
//! Everything here is safe Rust. Native executables use a small C
//! implementation of the same functions (`crates/brim-codegen/runtime`),
//! which must behave identically; the differential tests in `brim-cli` check
//! that they do.

pub mod abi;
pub mod ops;

use std::fmt;
use std::io::{self, Write};
use std::rc::Rc;

pub use abi::{AbiType, RuntimeFn};

/// The runtime representation of a Brim `string`: immutable, UTF-8,
/// reference-counted. Cloning is cheap and never copies the text.
pub type BrimStr = Rc<str>;

/// The default maximum number of simultaneously active Brim calls,
/// including `main`. Deeper recursion is the run-time error `R0003`. The
/// interpreter and native executables use the same limit, so both stop a
/// runaway recursion at the same point.
pub const MAX_CALL_DEPTH: usize = 10_000;

/// A failure that stops a running Brim program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    /// An `i64` operation produced a result outside the `i64` range.
    IntegerOverflow { op: &'static str },
    /// Integer division or remainder by zero.
    DivisionByZero { op: &'static str },
    /// The call stack grew beyond the configured limit.
    StackOverflow { limit: usize },
    /// Writing program output failed.
    Io { message: String },
    /// A list was indexed outside its bounds.
    IndexOutOfBounds { index: i64, len: i64 },
    /// A map was indexed with a key it does not have.
    KeyNotFound,
}

impl RuntimeError {
    /// The stable code for this error, documented in `docs/error-handling.md`.
    pub fn code(&self) -> &'static str {
        match self {
            RuntimeError::IntegerOverflow { .. } => "R0001",
            RuntimeError::DivisionByZero { .. } => "R0002",
            RuntimeError::StackOverflow { .. } => "R0003",
            RuntimeError::Io { .. } => "R0004",
            RuntimeError::IndexOutOfBounds { .. } => "R0005",
            RuntimeError::KeyNotFound => "R0006",
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::IntegerOverflow { op } => {
                write!(f, "integer overflow in `{op}`")
            }
            RuntimeError::DivisionByZero { op } => {
                let what = if *op == "%" { "remainder" } else { "division" };
                write!(f, "{what} by zero")
            }
            RuntimeError::StackOverflow { limit } => {
                write!(f, "stack overflow: more than {limit} nested calls")
            }
            RuntimeError::Io { message } => write!(f, "failed to write output: {message}"),
            RuntimeError::IndexOutOfBounds { index, len } => write!(
                f,
                "index out of bounds: the index is {index} but the length is {len}"
            ),
            RuntimeError::KeyNotFound => write!(f, "key not found in map"),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Implements `print(i64)`: writes the value in decimal and a newline.
pub fn print_i64(out: &mut dyn Write, value: i64) -> io::Result<()> {
    writeln!(out, "{value}")
}

/// Implements `print(bool)`: writes `true` or `false` and a newline.
pub fn print_bool(out: &mut dyn Write, value: bool) -> io::Result<()> {
    writeln!(out, "{value}")
}

/// Implements `print(string)`: writes the text and a newline.
pub fn print_str(out: &mut dyn Write, value: &str) -> io::Result<()> {
    writeln!(out, "{value}")
}

/// Implements string `+`: returns a new string.
pub fn str_concat(a: &str, b: &str) -> BrimStr {
    let mut s = String::with_capacity(a.len() + b.len());
    s.push_str(a);
    s.push_str(b);
    Rc::from(s)
}

/// Implements `to_string(i64)`: the decimal representation.
pub fn i64_to_str(value: i64) -> BrimStr {
    Rc::from(value.to_string())
}

/// Implements `to_string(bool)`: `"true"` or `"false"`.
pub fn bool_to_str(value: bool) -> BrimStr {
    Rc::from(if value { "true" } else { "false" })
}

/// Implements string `==`: byte-wise equality of the UTF-8 text.
pub fn str_eq(a: &str, b: &str) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printing() {
        let mut out = Vec::new();
        print_i64(&mut out, -42).unwrap();
        print_bool(&mut out, true).unwrap();
        print_str(&mut out, "héllo").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "-42\ntrue\nhéllo\n");
    }

    #[test]
    fn strings() {
        assert_eq!(&*str_concat("Hello, ", "Brim"), "Hello, Brim");
        assert_eq!(&*str_concat("", ""), "");
        assert!(str_eq("a", "a"));
        assert!(!str_eq("a", "A"));
        assert_eq!(&*i64_to_str(-9223372036854775808), "-9223372036854775808");
        assert_eq!(&*bool_to_str(false), "false");
    }

    #[test]
    fn error_codes_and_messages() {
        let e = RuntimeError::DivisionByZero { op: "%" };
        assert_eq!(e.code(), "R0002");
        assert_eq!(e.to_string(), "remainder by zero");
        assert_eq!(
            RuntimeError::IntegerOverflow { op: "*" }.to_string(),
            "integer overflow in `*`"
        );
    }
}
