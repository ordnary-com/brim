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
//! The string and I/O functions here define the behaviour the native runtime
//! must match exactly, including error messages.
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
    /// A string slice outside the string or not on character boundaries.
    InvalidSlice,
    /// `assert` or `assert_eq` failed; `assert_eq` reports both values (as
    /// Brim would write them).
    AssertionFailed { values: Option<(String, String)> },
    /// The program ran more instructions than an embedder allowed (see
    /// `brim_interp::Limits::max_steps`). Not used by `brim run` or native
    /// executables.
    StepLimit { limit: u64 },
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
            RuntimeError::InvalidSlice => "R0007",
            RuntimeError::AssertionFailed { .. } => "R0008",
            RuntimeError::StepLimit { .. } => "R0009",
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
            RuntimeError::AssertionFailed { values: None } => write!(f, "assertion failed"),
            RuntimeError::AssertionFailed { values: Some(_) } => {
                write!(f, "assertion failed: the values are not equal")
            }
            RuntimeError::InvalidSlice => write!(
                f,
                "invalid string slice: outside the string or not on a character boundary"
            ),
            RuntimeError::StepLimit { limit } => {
                write!(f, "step limit exceeded: more than {limit} instructions")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl RuntimeError {
    /// Notes shown with the error, before the call stack.
    pub fn notes(&self) -> Vec<String> {
        match self {
            RuntimeError::AssertionFailed {
                values: Some((left, right)),
            } => vec![format!("left:  {left}"), format!("right: {right}")],
            _ => Vec::new(),
        }
    }
}

/// A value as `assert_eq` shows it: strings in double quotes, as they are.
pub fn shown_str(s: &str) -> String {
    format!("\"{s}\"")
}

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

/// Writes a line to standard error (`eprint`).
pub fn eprint_str(err: &mut dyn Write, value: &str) -> io::Result<()> {
    writeln!(err, "{value}")
}

/// Byte-wise comparison: -1, 0 or 1 (string `<` and friends).
pub fn str_compare(a: &str, b: &str) -> i64 {
    match a.cmp(b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// The byte position of the first `pattern` in `s`, or -1.
pub fn str_find(s: &str, pattern: &str) -> i64 {
    s.find(pattern).map_or(-1, |i| i as i64)
}

/// Bytes `start..end` of `s`, which must be in the string and on character
/// boundaries.
pub fn str_slice(s: &str, start: i64, end: i64) -> Result<BrimStr, RuntimeError> {
    let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
        return Err(RuntimeError::InvalidSlice);
    };
    s.get(start..end)
        .map(BrimStr::from)
        .ok_or(RuntimeError::InvalidSlice)
}

/// The whitespace `trim` removes: space, tab, line feed, carriage return,
/// vertical tab and form feed.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

pub fn str_trim(s: &str) -> BrimStr {
    BrimStr::from(s.trim_matches(is_space))
}

pub fn str_trim_start(s: &str) -> BrimStr {
    BrimStr::from(s.trim_start_matches(is_space))
}

pub fn str_trim_end(s: &str) -> BrimStr {
    BrimStr::from(s.trim_end_matches(is_space))
}

/// ASCII letters in upper case; everything else unchanged.
pub fn str_to_upper(s: &str) -> BrimStr {
    BrimStr::from(s.to_ascii_uppercase())
}

/// ASCII letters in lower case; everything else unchanged.
pub fn str_to_lower(s: &str) -> BrimStr {
    BrimStr::from(s.to_ascii_lowercase())
}

/// Every non-overlapping `from` replaced by `to`. An empty `from` matches
/// before every character and at the end.
pub fn str_replace(s: &str, from: &str, to: &str) -> BrimStr {
    BrimStr::from(s.replace(from, to))
}

/// `n` copies of `s`; empty for `n <= 0`.
pub fn str_repeat(s: &str, n: i64) -> BrimStr {
    BrimStr::from(s.repeat(usize::try_from(n).unwrap_or(0)))
}

/// The parts of `s` between separators. An empty separator splits into
/// characters.
pub fn str_split(s: &str, sep: &str) -> Vec<BrimStr> {
    if sep.is_empty() {
        return str_chars(s);
    }
    s.split(sep).map(BrimStr::from).collect()
}

/// The lines of `s`, without line endings (`\n` or `\r\n`). A final line
/// ending does not start another line.
pub fn str_lines(s: &str) -> Vec<BrimStr> {
    s.lines().map(BrimStr::from).collect()
}

/// Each character of `s` as a string.
pub fn str_chars(s: &str) -> Vec<BrimStr> {
    s.chars()
        .map(|c| BrimStr::from(c.encode_utf8(&mut [0; 4]) as &str))
        .collect()
}

/// A decimal integer with an optional `+` or `-`, if `s` is exactly that
/// and fits in an `i64`.
pub fn str_parse_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// The message for a failed file operation (`verb` is "read" or "write"),
/// the same in the interpreter and in native executables.
pub fn file_error(verb: &str, path: &str, error: &io::Error) -> String {
    let reason = match error.kind() {
        io::ErrorKind::NotFound => "file not found",
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::IsADirectory => "is a directory",
        io::ErrorKind::InvalidData => "file is not valid UTF-8",
        _ => "input/output error",
    };
    format!("cannot {verb} file \"{path}\": {reason}")
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
