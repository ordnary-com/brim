//! The runtime ABI: functions that generated code calls into.
//!
//! Each [`RuntimeFn`] has a stable symbol name and a signature expressed in
//! [`AbiType`]s. The planned native lowering of each ABI type is:
//!
//! | `AbiType` | Brim type | Native representation                          |
//! |-----------|-----------|------------------------------------------------|
//! | `I64`     | `i64`     | 64-bit two's complement integer                |
//! | `Bool`    | `bool`    | 8-bit integer, `0` or `1`                      |
//! | `Str`     | `string`  | pointer to runtime-owned string object (opaque)|
//! | `Void`    | `void`    | no value                                       |
//!
//! Adding a variant is backwards compatible; changing an existing symbol name
//! or signature is a breaking change to the ABI.

use std::fmt;

/// Types that can cross the boundary between generated code and the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AbiType {
    I64,
    Bool,
    Str,
    Void,
}

impl fmt::Display for AbiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AbiType::I64 => "i64",
            AbiType::Bool => "bool",
            AbiType::Str => "str",
            AbiType::Void => "void",
        })
    }
}

/// A function provided by the Brim runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeFn {
    PrintI64,
    PrintBool,
    PrintStr,
    StrConcat,
    StrEq,
}

impl RuntimeFn {
    /// Every runtime function, in a stable order.
    pub const ALL: &'static [RuntimeFn] = &[
        RuntimeFn::PrintI64,
        RuntimeFn::PrintBool,
        RuntimeFn::PrintStr,
        RuntimeFn::StrConcat,
        RuntimeFn::StrEq,
    ];

    /// The linker symbol a native backend will call.
    pub fn symbol(self) -> &'static str {
        match self {
            RuntimeFn::PrintI64 => "brim_rt_print_i64",
            RuntimeFn::PrintBool => "brim_rt_print_bool",
            RuntimeFn::PrintStr => "brim_rt_print_str",
            RuntimeFn::StrConcat => "brim_rt_str_concat",
            RuntimeFn::StrEq => "brim_rt_str_eq",
        }
    }

    pub fn params(self) -> &'static [AbiType] {
        match self {
            RuntimeFn::PrintI64 => &[AbiType::I64],
            RuntimeFn::PrintBool => &[AbiType::Bool],
            RuntimeFn::PrintStr => &[AbiType::Str],
            RuntimeFn::StrConcat | RuntimeFn::StrEq => &[AbiType::Str, AbiType::Str],
        }
    }

    pub fn ret(self) -> AbiType {
        match self {
            RuntimeFn::PrintI64 | RuntimeFn::PrintBool | RuntimeFn::PrintStr => AbiType::Void,
            RuntimeFn::StrConcat => AbiType::Str,
            RuntimeFn::StrEq => AbiType::Bool,
        }
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
