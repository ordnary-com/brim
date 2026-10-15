//! The Brim standard library catalogue.
//!
//! For now the standard library consists only of *builtin functions*: names
//! that are always in scope and are implemented by the runtime rather than in
//! Brim source. This crate is the single place that declares them, so the type
//! checker, documentation and future tooling (editor completion, docs
//! generation) agree on what exists.
//!
//! A builtin may have several *overloads* that differ in parameter types; each
//! overload maps to exactly one [`RuntimeFn`]. User-defined functions cannot be
//! overloaded.
//!
//! When Brim gains modules, this catalogue is expected to become the `std`
//! prelude, with most of the library written in Brim itself.

use std::fmt;

use brim_runtime::RuntimeFn;

/// The primitive types the standard library is described in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    I64,
    I32,
    U8,
    U64,
    F64,
    Bool,
    String,
    Void,
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Prim::I64 => "i64",
            Prim::I32 => "i32",
            Prim::U8 => "u8",
            Prim::U64 => "u64",
            Prim::F64 => "f64",
            Prim::Bool => "bool",
            Prim::String => "string",
            Prim::Void => "void",
        })
    }
}

/// One signature of a builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overload {
    pub params: &'static [Prim],
    pub ret: Prim,
    /// The runtime function that implements this overload.
    pub runtime: RuntimeFn,
}

/// A builtin function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    /// One-line description for documentation and tooling.
    pub summary: &'static str,
    pub overloads: &'static [Overload],
}

impl Builtin {
    /// Selects the overload whose parameter types exactly match `args`.
    pub fn resolve(&self, args: &[Prim]) -> Option<&'static Overload> {
        self.overloads.iter().find(|o| o.params == args)
    }

    /// The distinct numbers of parameters this builtin accepts.
    pub fn arities(&self) -> Vec<usize> {
        let mut arities: Vec<_> = self.overloads.iter().map(|o| o.params.len()).collect();
        arities.sort_unstable();
        arities.dedup();
        arities
    }
}

/// Every builtin function.
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "print",
        summary: "Writes a value followed by a newline to standard output.",
        overloads: &[
            Overload {
                params: &[Prim::String],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintStr,
            },
            Overload {
                params: &[Prim::I64],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintI64,
            },
            Overload {
                params: &[Prim::Bool],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintBool,
            },
            Overload {
                params: &[Prim::F64],
                ret: Prim::Void,
                runtime: RuntimeFn::PrintF64,
            },
        ],
    },
    Builtin {
        name: "eprint",
        summary: "Writes a string followed by a newline to standard error.",
        overloads: &[Overload {
            params: &[Prim::String],
            ret: Prim::Void,
            runtime: RuntimeFn::EPrintStr,
        }],
    },
    Builtin {
        name: "exit",
        summary: "Ends the program with an exit code (0 to 255).",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::Void,
            runtime: RuntimeFn::Exit,
        }],
    },
    Builtin {
        name: "assert",
        summary: "Stops the program with an error if the condition is false.",
        overloads: &[Overload {
            params: &[Prim::Bool],
            ret: Prim::Void,
            runtime: RuntimeFn::Assert,
        }],
    },
    Builtin {
        name: "assert_eq",
        summary: "Stops the program with an error if two values differ.",
        overloads: &[
            Overload {
                params: &[Prim::I64, Prim::I64],
                ret: Prim::Void,
                runtime: RuntimeFn::AssertEqI64,
            },
            Overload {
                params: &[Prim::Bool, Prim::Bool],
                ret: Prim::Void,
                runtime: RuntimeFn::AssertEqBool,
            },
            Overload {
                params: &[Prim::String, Prim::String],
                ret: Prim::Void,
                runtime: RuntimeFn::AssertEqStr,
            },
        ],
    },
    Builtin {
        name: "to_string",
        summary: "Converts a value to its textual representation.",
        overloads: &[
            Overload {
                params: &[Prim::I64],
                ret: Prim::String,
                runtime: RuntimeFn::I64ToStr,
            },
            Overload {
                params: &[Prim::Bool],
                ret: Prim::String,
                runtime: RuntimeFn::BoolToStr,
            },
            Overload {
                params: &[Prim::F64],
                ret: Prim::String,
                runtime: RuntimeFn::F64ToStr,
            },
        ],
    },
    Builtin {
        name: "to_float",
        summary: "Converts an integer to the nearest floating-point number.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::F64,
            runtime: RuntimeFn::I64ToF64,
        }],
    },
    Builtin {
        name: "to_int",
        summary: "Converts a floating-point number to an integer, rounding toward zero.",
        overloads: &[Overload {
            params: &[Prim::F64],
            ret: Prim::I64,
            runtime: RuntimeFn::F64ToI64,
        }],
    },
    // Primitives for the standard library modules (`std.net`, ...). Names
    // starting with `__` can only be used inside `std` modules.
    Builtin {
        name: "__last_error",
        summary: "The error of the last runtime call, or an empty string.",
        overloads: &[Overload {
            params: &[],
            ret: Prim::String,
            runtime: RuntimeFn::LastError,
        }],
    },
    Builtin {
        name: "__net_listen",
        summary: "std.net: listen on a host and port.",
        overloads: &[Overload {
            params: &[Prim::String, Prim::I64],
            ret: Prim::I64,
            runtime: RuntimeFn::NetListen,
        }],
    },
    Builtin {
        name: "__net_accept",
        summary: "std.net: accept a connection.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::I64,
            runtime: RuntimeFn::NetAccept,
        }],
    },
    Builtin {
        name: "__net_connect",
        summary: "std.net: connect to a host and port.",
        overloads: &[Overload {
            params: &[Prim::String, Prim::I64],
            ret: Prim::I64,
            runtime: RuntimeFn::NetConnect,
        }],
    },
    Builtin {
        name: "__net_read_until",
        summary: "std.net: read up to a delimiter.",
        overloads: &[Overload {
            params: &[Prim::I64, Prim::String, Prim::I64],
            ret: Prim::String,
            runtime: RuntimeFn::NetReadUntil,
        }],
    },
    Builtin {
        name: "__net_read_exact",
        summary: "std.net: read a number of bytes.",
        overloads: &[Overload {
            params: &[Prim::I64, Prim::I64],
            ret: Prim::String,
            runtime: RuntimeFn::NetReadExact,
        }],
    },
    Builtin {
        name: "__net_read_all",
        summary: "std.net: read until the connection closes.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::String,
            runtime: RuntimeFn::NetReadAll,
        }],
    },
    Builtin {
        name: "__net_write",
        summary: "std.net: write text.",
        overloads: &[Overload {
            params: &[Prim::I64, Prim::String],
            ret: Prim::I64,
            runtime: RuntimeFn::NetWrite,
        }],
    },
    Builtin {
        name: "__net_close",
        summary: "std.net: close a socket.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::Void,
            runtime: RuntimeFn::NetClose,
        }],
    },
    Builtin {
        name: "__net_local_port",
        summary: "std.net: the local port of a socket.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::I64,
            runtime: RuntimeFn::NetLocalPort,
        }],
    },
    Builtin {
        name: "__url_decode",
        summary: "std.http: percent-decode text.",
        overloads: &[Overload {
            params: &[Prim::String, Prim::Bool],
            ret: Prim::String,
            runtime: RuntimeFn::UrlDecode,
        }],
    },
    Builtin {
        name: "__url_encode",
        summary: "std.http: percent-encode text.",
        overloads: &[Overload {
            params: &[Prim::String],
            ret: Prim::String,
            runtime: RuntimeFn::UrlEncode,
        }],
    },
    Builtin {
        name: "__char_from_code",
        summary: "std.json: the character with a code point.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::String,
            runtime: RuntimeFn::CharFromCode,
        }],
    },
    Builtin {
        name: "__char_code",
        summary: "std.json: the code point of a character.",
        overloads: &[Overload {
            params: &[Prim::String],
            ret: Prim::I64,
            runtime: RuntimeFn::CharCode,
        }],
    },
    Builtin {
        name: "__http_request",
        summary: "std.http: an HTTPS request through libcurl.",
        overloads: &[Overload {
            params: &[
                Prim::String,
                Prim::String,
                Prim::String,
                Prim::String,
                Prim::I64,
            ],
            ret: Prim::String,
            runtime: RuntimeFn::HttpRequest,
        }],
    },
    Builtin {
        name: "__time_now_millis",
        summary: "std.time: the wall clock in milliseconds.",
        overloads: &[Overload {
            params: &[],
            ret: Prim::I64,
            runtime: RuntimeFn::TimeNowMillis,
        }],
    },
    Builtin {
        name: "__time_monotonic_millis",
        summary: "std.time: a monotonic clock in milliseconds.",
        overloads: &[Overload {
            params: &[],
            ret: Prim::I64,
            runtime: RuntimeFn::TimeMonotonicMillis,
        }],
    },
    Builtin {
        name: "__time_sleep_millis",
        summary: "std.time: wait some milliseconds.",
        overloads: &[Overload {
            params: &[Prim::I64],
            ret: Prim::Void,
            runtime: RuntimeFn::TimeSleepMillis,
        }],
    },
    Builtin {
        name: "__net_set_timeout",
        summary: "std.net: set a read and write timeout.",
        overloads: &[Overload {
            params: &[Prim::I64, Prim::I64],
            ret: Prim::Void,
            runtime: RuntimeFn::NetSetTimeout,
        }],
    },
];

/// Whether a builtin is reserved for the standard library modules.
pub fn is_internal(name: &str) -> bool {
    name.starts_with("__")
}

/// Looks up a builtin by name.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_runtime::AbiType;

    fn abi(p: Prim) -> AbiType {
        match p {
            Prim::I64 | Prim::I32 | Prim::U8 | Prim::U64 => AbiType::I64,
            Prim::F64 => AbiType::F64,
            Prim::Bool => AbiType::Bool,
            Prim::String => AbiType::Str,
            Prim::Void => AbiType::Void,
        }
    }

    #[test]
    fn print_resolves_by_argument_type() {
        let print = lookup("print").unwrap();
        assert_eq!(
            print.resolve(&[Prim::I64]).unwrap().runtime,
            RuntimeFn::PrintI64
        );
        assert_eq!(
            print.resolve(&[Prim::String]).unwrap().runtime,
            RuntimeFn::PrintStr
        );
        assert!(print.resolve(&[Prim::Void]).is_none());
        assert!(print.resolve(&[]).is_none());
        assert_eq!(print.arities(), vec![1]);
    }

    #[test]
    fn overloads_match_runtime_signatures() {
        for builtin in BUILTINS {
            for overload in builtin.overloads {
                let params: Vec<_> = overload.params.iter().map(|p| abi(*p)).collect();
                assert_eq!(params, overload.runtime.params(), "{}", builtin.name);
                assert_eq!(
                    abi(overload.ret),
                    overload.runtime.ret(),
                    "{}",
                    builtin.name
                );
            }
        }
    }

    #[test]
    fn to_string_resolves() {
        let to_string = lookup("to_string").unwrap();
        assert_eq!(to_string.resolve(&[Prim::I64]).unwrap().ret, Prim::String);
        assert!(to_string.resolve(&[Prim::String]).is_none());
    }

    #[test]
    fn unknown_names() {
        assert!(lookup("println").is_none());
    }
}
