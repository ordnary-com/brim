//! The runtime ABI: functions that generated code calls into.
//!
//! Each [`RuntimeFn`] has a stable symbol name and a signature expressed in
//! [`AbiType`]s. The native lowering of each ABI type is:
//!
//! | `AbiType` | Brim type      | Native representation                    |
//! |-----------|----------------|------------------------------------------|
//! | `I64`     | `i64`          | 64-bit two's complement integer          |
//! | `Bool`    | `bool`         | 8-bit integer, `0` or `1`                |
//! | `Str`     | `string`       | pointer to a runtime string object       |
//! | `StrList` | `List[string]` | pointer to a runtime list of strings     |
//! | `Void`    | `void`         | no value                                 |
//!
//! Functions that can fail ([`RuntimeFn::failure`]) take three more pointer
//! arguments, the pre-rendered error message (see `brim-codegen`).
//!
//! Some I/O functions report errors through [`RuntimeFn::LastError`] instead:
//! after them, `LastError` returns an empty string on success and a message
//! otherwise.
//!
//! Adding a variant is backwards compatible; changing an existing symbol name
//! or signature is a breaking change to the ABI.

use std::fmt;

use crate::RuntimeError;

/// Types that can cross the boundary between generated code and the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AbiType {
    I64,
    Bool,
    Str,
    /// `List[string]`.
    StrList,
    Void,
}

impl fmt::Display for AbiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AbiType::I64 => "i64",
            AbiType::Bool => "bool",
            AbiType::Str => "str",
            AbiType::StrList => "[str]",
            AbiType::Void => "void",
        })
    }
}

macro_rules! runtime_fns {
    ($($(#[$doc:meta])* $name:ident = $symbol:literal ($($param:ident),*) -> $ret:ident;)*) => {
        /// A function provided by the Brim runtime.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum RuntimeFn {
            $($(#[$doc])* $name,)*
        }

        impl RuntimeFn {
            /// Every runtime function, in a stable order.
            pub const ALL: &'static [RuntimeFn] = &[$(RuntimeFn::$name),*];

            /// The linker symbol a native backend calls.
            pub fn symbol(self) -> &'static str {
                match self {
                    $(RuntimeFn::$name => $symbol,)*
                }
            }

            pub fn params(self) -> &'static [AbiType] {
                match self {
                    $(RuntimeFn::$name => &[$(AbiType::$param),*],)*
                }
            }

            pub fn ret(self) -> AbiType {
                match self {
                    $(RuntimeFn::$name => AbiType::$ret,)*
                }
            }
        }
    };
}

runtime_fns! {
    /// `print(i64)`
    PrintI64 = "brim_rt_print_i64" (I64) -> Void;
    /// `print(bool)`
    PrintBool = "brim_rt_print_bool" (Bool) -> Void;
    /// `print(string)`
    PrintStr = "brim_rt_print_str" (Str) -> Void;
    /// `eprint(string)`: a line on standard error.
    EPrintStr = "brim_rt_eprint_str" (Str) -> Void;
    /// String `+`.
    StrConcat = "brim_rt_str_concat" (Str, Str) -> Str;
    /// String `==`.
    StrEq = "brim_rt_str_eq" (Str, Str) -> Bool;
    /// Byte-wise comparison: -1, 0 or 1.
    StrCompare = "brim_rt_str_compare" (Str, Str) -> I64;
    /// `to_string(i64)`
    I64ToStr = "brim_rt_i64_to_str" (I64) -> Str;
    /// `to_string(bool)`
    BoolToStr = "brim_rt_bool_to_str" (Bool) -> Str;
    /// `s.len()`: the length in bytes.
    StrLen = "brim_rt_str_len" (Str) -> I64;
    /// `s.contains(t)`
    StrContains = "brim_rt_str_contains" (Str, Str) -> Bool;
    /// `s.starts_with(t)`
    StrStartsWith = "brim_rt_str_starts_with" (Str, Str) -> Bool;
    /// `s.ends_with(t)`
    StrEndsWith = "brim_rt_str_ends_with" (Str, Str) -> Bool;
    /// The byte position of the first `t` in `s`, or -1.
    StrFind = "brim_rt_str_find" (Str, Str) -> I64;
    /// `s.slice(start, end)`: bytes `start..end`, which must be in the
    /// string and on character boundaries.
    StrSlice = "brim_rt_str_slice" (Str, I64, I64) -> Str;
    /// `s.trim()`: without leading and trailing whitespace.
    StrTrim = "brim_rt_str_trim" (Str) -> Str;
    StrTrimStart = "brim_rt_str_trim_start" (Str) -> Str;
    StrTrimEnd = "brim_rt_str_trim_end" (Str) -> Str;
    /// ASCII letters in upper case.
    StrToUpper = "brim_rt_str_to_upper" (Str) -> Str;
    /// ASCII letters in lower case.
    StrToLower = "brim_rt_str_to_lower" (Str) -> Str;
    /// `s.replace(from, to)`: every non-overlapping `from` replaced.
    StrReplace = "brim_rt_str_replace" (Str, Str, Str) -> Str;
    /// `s.repeat(n)`; empty for `n <= 0`.
    StrRepeat = "brim_rt_str_repeat" (Str, I64) -> Str;
    /// `s.split(sep)`; an empty separator splits into characters.
    StrSplit = "brim_rt_str_split" (Str, Str) -> StrList;
    /// `s.lines()`
    StrLines = "brim_rt_str_lines" (Str) -> StrList;
    /// `s.chars()`: each character as a string.
    StrChars = "brim_rt_str_chars" (Str) -> StrList;
    /// Whether `s` is a decimal `i64`, with an optional sign.
    StrParseIntOk = "brim_rt_str_parse_int_ok" (Str) -> Bool;
    /// The value of a string for which `StrParseIntOk` is true.
    StrParseInt = "brim_rt_str_parse_int" (Str) -> I64;
    /// A line from standard input, without its line ending. Sets the last
    /// error at the end of input.
    ReadLine = "brim_rt_read_line" () -> Str;
    /// A file's contents. Sets the last error on failure.
    ReadFile = "brim_rt_read_file" (Str) -> Str;
    /// Writes a file; returns the number of bytes. Sets the last error on
    /// failure.
    WriteFile = "brim_rt_write_file" (Str, Str) -> I64;
    /// The error of the last I/O function, or an empty string.
    LastError = "brim_rt_last_error" () -> Str;
    /// The program's command-line arguments, without the program name.
    Args = "brim_rt_args" () -> StrList;
    /// Whether an environment variable is set.
    EnvHas = "brim_rt_env_has" (Str) -> Bool;
    /// An environment variable's value (empty if it is not set).
    EnvGet = "brim_rt_env_get" (Str) -> Str;
    /// Ends the program with an exit code (0 to 255).
    Exit = "brim_rt_exit" (I64) -> Void;
    /// `assert(cond)`
    Assert = "brim_rt_assert" (Bool) -> Void;
    /// `assert_eq(a, b)` on integers.
    AssertEqI64 = "brim_rt_assert_eq_i64" (I64, I64) -> Void;
    /// `assert_eq(a, b)` on booleans.
    AssertEqBool = "brim_rt_assert_eq_bool" (Bool, Bool) -> Void;
    /// `assert_eq(a, b)` on strings.
    AssertEqStr = "brim_rt_assert_eq_str" (Str, Str) -> Void;
    /// Network functions for the `std.net` module. Sockets are handles
    /// (positive integers); every function sets the last error, with a
    /// short reason such as "connection refused" on failure.
    ///
    /// A listening socket on a host and port (0 picks a free port).
    NetListen = "brim_rt_net_listen" (Str, I64) -> I64;
    /// The next connection to a listening socket.
    NetAccept = "brim_rt_net_accept" (I64) -> I64;
    /// A connection to a host and port.
    NetConnect = "brim_rt_net_connect" (Str, I64) -> I64;
    /// The text up to a delimiter, which is read and dropped.
    NetReadUntil = "brim_rt_net_read_until" (I64, Str) -> Str;
    /// Exactly this many bytes, as text.
    NetReadExact = "brim_rt_net_read_exact" (I64, I64) -> Str;
    /// Everything until the other side closes the connection.
    NetReadAll = "brim_rt_net_read_all" (I64) -> Str;
    /// Writes text; returns the number of bytes.
    NetWrite = "brim_rt_net_write" (I64, Str) -> I64;
    /// Closes a socket. Closing an unknown or closed handle does nothing.
    NetClose = "brim_rt_net_close" (I64) -> Void;
    /// The local port of a socket.
    NetLocalPort = "brim_rt_net_local_port" (I64) -> I64;
    /// Read and write timeout in milliseconds; 0 for none.
    NetSetTimeout = "brim_rt_net_set_timeout" (I64, I64) -> Void;
    /// Percent-decodes text (`a%20b` and, with the flag, `a+b` to `a b`).
    /// Sets the last error if the result is not UTF-8 or an escape is bad.
    UrlDecode = "brim_rt_url_decode" (Str, Bool) -> Str;
    /// Percent-encodes everything but unreserved characters
    /// (`A-Z a-z 0-9 - . _ ~`).
    UrlEncode = "brim_rt_url_encode" (Str) -> Str;
    /// The character with a Unicode code point, as a string. Sets the last
    /// error for surrogates and values outside Unicode.
    CharFromCode = "brim_rt_char_from_code" (I64) -> Str;
    /// The code point of the first character of a string; -1 if empty.
    CharCode = "brim_rt_char_code" (Str) -> I64;
}

impl RuntimeFn {
    /// The run-time error this function can stop the program with, if any.
    pub fn failure(self) -> Option<RuntimeError> {
        match self {
            RuntimeFn::StrSlice => Some(RuntimeError::InvalidSlice),
            RuntimeFn::Assert => Some(RuntimeError::AssertionFailed { values: None }),
            // The values are printed by the runtime.
            RuntimeFn::AssertEqI64 | RuntimeFn::AssertEqBool | RuntimeFn::AssertEqStr => {
                Some(RuntimeError::AssertionFailed {
                    values: Some(Default::default()),
                })
            }
            _ => None,
        }
    }

    /// Whether the function never returns.
    pub fn diverges(self) -> bool {
        self == RuntimeFn::Exit
    }
}

impl fmt::Display for RuntimeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn symbols_are_unique_and_prefixed() {
        let symbols: HashSet<_> = RuntimeFn::ALL.iter().map(|f| f.symbol()).collect();
        assert_eq!(symbols.len(), RuntimeFn::ALL.len());
        assert!(symbols.iter().all(|s| s.starts_with("brim_rt_")));
    }

    #[test]
    fn void_never_appears_as_a_parameter() {
        for f in RuntimeFn::ALL {
            assert!(!f.params().contains(&AbiType::Void), "{f}");
        }
    }
}
