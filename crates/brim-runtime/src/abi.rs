//! The runtime ABI: functions that generated code calls into.
//!
//! Each [`RuntimeFn`] has a stable symbol name and a signature expressed in
//! [`AbiType`]s. The native lowering of each ABI type is:
//!
//! | `AbiType` | Brim type      | Native representation                    |
//! |-----------|----------------|------------------------------------------|
//! | `I64`     | `i64`          | 64-bit two's complement integer          |
//! | `F64`     | `f64`          | IEEE 754 double                          |
//! | `Bool`    | `bool`         | 8-bit integer, `0` or `1`                |
//! | `Str`     | `string`       | pointer to a runtime string object       |
//! | `StrList` | `List[string]` | pointer to a runtime list of strings     |
//! | `IntList` | `List[u8]`     | pointer to a runtime list of `i64`s      |
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
    F64,
    Bool,
    Str,
    /// `List[string]`.
    StrList,
    /// A list of integers held as `i64` (`List[u8]` for bytes).
    IntList,
    Void,
}

impl fmt::Display for AbiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AbiType::I64 => "i64",
            AbiType::F64 => "f64",
            AbiType::Bool => "bool",
            AbiType::Str => "str",
            AbiType::StrList => "[str]",
            AbiType::IntList => "[i64]",
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
    /// `print(f64)`
    PrintF64 = "brim_rt_print_f64" (F64) -> Void;
    /// `to_string(f64)`: the fewest digits that read back exactly.
    F64ToStr = "brim_rt_f64_to_str" (F64) -> Str;
    /// `to_float(i64)`: the nearest `f64`.
    I64ToF64 = "brim_rt_i64_to_f64" (I64) -> F64;
    /// `to_int(f64)`: rounds toward zero; NaN and numbers outside `i64`
    /// are run-time errors.
    F64ToI64 = "brim_rt_f64_to_i64" (F64) -> I64;
    /// Whether `s` is a decimal number for `parse_float`.
    StrParseFloatOk = "brim_rt_str_parse_float_ok" (Str) -> Bool;
    /// The value of a string for which `StrParseFloatOk` is true.
    StrParseFloat = "brim_rt_str_parse_float" (Str) -> F64;
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
    /// `abs(i64)`; `abs(i64::MIN)` does not fit.
    AbsI64 = "brim_rt_abs_i64" (I64) -> I64;
    /// `abs(f64)`
    AbsF64 = "brim_rt_abs_f64" (F64) -> F64;
    /// `sqrt(f64)`: NaN for negative numbers.
    Sqrt = "brim_rt_sqrt" (F64) -> F64;
    /// `floor(f64)`
    Floor = "brim_rt_floor" (F64) -> F64;
    /// `ceil(f64)`
    Ceil = "brim_rt_ceil" (F64) -> F64;
    /// `round(f64)`: halfway cases away from zero.
    Round = "brim_rt_round" (F64) -> F64;
    /// `pow(f64, f64)`
    PowF64 = "brim_rt_pow_f64" (F64, F64) -> F64;
    /// `pow(i64, i64)`: a negative exponent divides, rounding toward zero
    /// (`pow(2, -1)` is 0); a result outside `i64`, or `pow(0, -1)`, fails.
    PowI64 = "brim_rt_pow_i64" (I64, I64) -> I64;
    /// `to_string(u64)`: the bits of the `i64` read as unsigned.
    U64ToStr = "brim_rt_u64_to_str" (I64) -> Str;
    /// `to_float(u64)`
    U64ToF64 = "brim_rt_u64_to_f64" (I64) -> F64;
    /// `PrintStr` of `U64ToStr`.
    PrintU64 = "brim_rt_print_u64" (I64) -> Void;
    /// A string as a Brim string literal, for printing values that
    /// contain strings: `"a\"b"`.
    StrQuote = "brim_rt_str_quote" (Str) -> Str;
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
    /// `s.code_points()`: the Unicode code point of each character.
    StrCodePoints = "brim_rt_str_code_points" (Str) -> IntList;
    /// Whether `n` is a Unicode scalar value, for `from_code_point`.
    CodePointOk = "brim_rt_code_point_ok" (I64) -> Bool;
    /// The character with a code point for which `CodePointOk` is true.
    CodePointToStr = "brim_rt_code_point_to_str" (I64) -> Str;
    /// `s.bytes()`: the UTF-8 bytes of a string.
    StrBytes = "brim_rt_str_bytes" (Str) -> IntList;
    /// Whether bytes are valid UTF-8, for `string_from_bytes`.
    BytesUtf8Ok = "brim_rt_bytes_utf8_ok" (IntList) -> Bool;
    /// The string of bytes for which `BytesUtf8Ok` is true.
    BytesToStr = "brim_rt_bytes_to_str" (IntList) -> Str;
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
    /// The text up to a delimiter, which is read and dropped; at most this
    /// many bytes before it ("message too large" otherwise).
    NetReadUntil = "brim_rt_net_read_until" (I64, Str, I64) -> Str;
    /// Exactly this many bytes, as text.
    NetReadExact = "brim_rt_net_read_exact" (I64, I64) -> Str;
    /// Everything until the other side closes the connection.
    NetReadAll = "brim_rt_net_read_all" (I64) -> Str;
    /// Writes text; returns the number of bytes.
    NetWrite = "brim_rt_net_write" (I64, Str) -> I64;
    /// Closes a socket. Closing an unknown or closed handle does nothing.
    NetClose = "brim_rt_net_close" (I64) -> Void;
    /// Gives a connection back to its listener to wait for more (HTTP
    /// keep-alive); at most 256 per listener, oldest closed first.
    NetKeep = "brim_rt_net_keep" (I64, I64) -> Void;
    /// The next connection of a listener with something to read: a kept
    /// one, or a new one. Kept connections quiet for the given milliseconds
    /// are closed. Sets the last error.
    NetNextReady = "brim_rt_net_next_ready" (I64, I64) -> I64;
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
    /// Milliseconds since 1970-01-01T00:00:00Z (the wall clock).
    TimeNowMillis = "brim_rt_time_now_millis" () -> I64;
    /// std.fs: whether a file or directory exists.
    FsExists = "brim_rt_fs_exists" (Str) -> Bool;
    /// std.fs: whether a path is a directory.
    FsIsDir = "brim_rt_fs_is_dir" (Str) -> Bool;
    /// std.fs: the names in a directory, sorted. Sets the last error.
    FsList = "brim_rt_fs_list" (Str) -> StrList;
    /// std.fs: creates a directory and its parents; false if it existed.
    /// Sets the last error.
    FsCreateDir = "brim_rt_fs_create_dir" (Str) -> Bool;
    /// std.fs: removes a file or an empty directory; false if it did not
    /// exist. Sets the last error.
    FsRemove = "brim_rt_fs_remove" (Str) -> Bool;
    /// std.fs: removes a file, or a directory with everything in it; false
    /// if it did not exist. Sets the last error.
    FsRemoveAll = "brim_rt_fs_remove_all" (Str) -> Bool;
    /// std.fs: renames or moves a file or directory. Sets the last error.
    FsRename = "brim_rt_fs_rename" (Str, Str) -> Bool;
    /// std.fs: a file's size in bytes. Sets the last error.
    FsSize = "brim_rt_fs_size" (Str) -> I64;
    /// std.fs: when a file was last changed, in Unix milliseconds. Sets the
    /// last error.
    FsModified = "brim_rt_fs_modified" (Str) -> I64;
    /// std.fs: adds text at the end of a file, creating it if needed; the
    /// number of bytes. Sets the last error.
    FsAppend = "brim_rt_fs_append" (Str, Str) -> I64;
    /// std.process: runs a program with arguments, with no input, and waits
    /// for it; the exit code, or 128 plus the signal that stopped it. Keeps
    /// what it wrote for `ProcessStdout` and `ProcessStderr`. Sets the last
    /// error if it cannot start or writes text that is not UTF-8.
    ProcessRun = "brim_rt_process_run" (Str, StrList) -> I64;
    /// std.process: the standard output of the last `ProcessRun`.
    ProcessStdout = "brim_rt_process_stdout" () -> Str;
    /// std.process: the standard error of the last `ProcessRun`.
    ProcessStderr = "brim_rt_process_stderr" () -> Str;
    /// A sorted copy of a list of signed integers (`sorted()`).
    ListSortI64 = "brim_rt_list_sort_i64" (IntList) -> IntList;
    /// A sorted copy of a list of `u64`s, compared without sign.
    ListSortU64 = "brim_rt_list_sort_u64" (IntList) -> IntList;
    /// A sorted copy of a list of strings, in byte order like `<`.
    ListSortStr = "brim_rt_list_sort_str" (StrList) -> StrList;
    /// std.crypto: the SHA-256 hash of bytes.
    Sha256 = "brim_rt_sha256" (IntList) -> IntList;
    /// std.crypto: HMAC-SHA-256 of a message (second) with a key (first).
    HmacSha256 = "brim_rt_hmac_sha256" (IntList, IntList) -> IntList;
    /// std.crypto: PBKDF2-HMAC-SHA-256 of a password and a salt, with a
    /// number of iterations and a length in bytes.
    Pbkdf2Sha256 = "brim_rt_pbkdf2_sha256" (IntList, IntList, I64, I64) -> IntList;
    /// std.crypto: the MD5 hash of bytes.
    Md5 = "brim_rt_md5" (IntList) -> IntList;
    /// std.crypto: bytes from the operating system's secure random source.
    RandomBytes = "brim_rt_random_bytes" (I64) -> IntList;
    /// std.net: exactly this many bytes. Sets the last error.
    NetReadBytes = "brim_rt_net_read_bytes" (I64, I64) -> IntList;
    /// std.net: writes bytes; returns how many. Sets the last error.
    NetWriteBytes = "brim_rt_net_write_bytes" (I64, IntList) -> I64;
    /// std.random: make the numbers that follow repeatable.
    RandomSeed = "brim_rt_random_seed" (I64) -> Void;
    /// std.random: a number from the first argument up to the second.
    RandomRange = "brim_rt_random_range" (I64, I64) -> I64;
    /// std.random: a number from 0 up to 1.
    RandomFloat = "brim_rt_random_float" () -> F64;
    /// Milliseconds on a clock that never goes back, for durations.
    TimeMonotonicMillis = "brim_rt_time_monotonic_millis" () -> I64;
    /// Waits this many milliseconds.
    TimeSleepMillis = "brim_rt_time_sleep_millis" (I64) -> Void;
    /// An HTTP request through libcurl, for `https://` URLs: method, URL,
    /// header lines (`name: value\r\n` each), body and a timeout in
    /// milliseconds. Returns the status code, the response's header lines
    /// and the body: `200\r\nname: value\r\n\r\nbody`. Sets the last
    /// error with a short reason on failure.
    HttpRequest = "brim_rt_http_request" (Str, Str, Str, Str, I64) -> Str;
}

impl RuntimeFn {
    /// The run-time error this function can stop the program with, if any.
    pub fn failure(self) -> Option<RuntimeError> {
        match self {
            RuntimeFn::StrSlice => Some(RuntimeError::InvalidSlice),
            RuntimeFn::F64ToI64 => Some(RuntimeError::FloatToInt),
            RuntimeFn::AbsI64 => Some(RuntimeError::IntegerOverflow { op: "abs" }),
            RuntimeFn::PowI64 => Some(RuntimeError::IntegerOverflow { op: "pow" }),
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
