//! The interface between the Brim compiler and native code generators.
//!
//! A [`Backend`] turns a validated IR [`Module`] into build artifacts on disk.
//! The frontend never talks to LLVM (or any other code generator) directly;
//! it only sees this trait. That keeps LLVM-specific code isolated in one
//! place and means the frontend, its tests and CI never require an LLVM
//! installation.
//!
//! # Status
//!
//! **No native backend is implemented yet.** [`LlvmBackend`] is the planned
//! backend; today it validates the module and then returns
//! [`CodegenError::BackendUnavailable`]. See `docs/architecture.md` for the
//! LLVM integration plan (LLVM 22 via the `inkwell` crate, behind an opt-in
//! Cargo feature). Brim programs can be executed with the reference
//! interpreter (`brim run`) in the meantime.

use std::fmt;
use std::path::PathBuf;

use brim_ir::{Module, ValidationError};

/// How much the backend should optimise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptLevel {
    #[default]
    Debug,
    Release,
}

/// What to build.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Path of the executable to produce.
    pub output: PathBuf,
    pub opt_level: OptLevel,
}

/// What a successful build produced.
#[derive(Clone, Debug)]
pub struct BuildOutput {
    pub artifacts: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodegenError {
    /// The backend is not available in this build of the compiler.
    BackendUnavailable {
        backend: &'static str,
        reason: String,
    },
    /// The module failed IR validation; this is a compiler bug.
    InvalidModule(Vec<ValidationError>),
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::BackendUnavailable { backend, reason } => {
                write!(f, "the {backend} backend is not available: {reason}")
            }
            CodegenError::InvalidModule(errors) => {
                writeln!(f, "internal compiler error: invalid IR")?;
                for e in errors {
                    writeln!(f, "  {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// A native code generator.
pub trait Backend {
    /// Short name used in messages, e.g. `"LLVM"`.
    fn name(&self) -> &'static str;

    /// Compiles `module` according to `options`.
    fn build(&self, module: &Module, options: &BuildOptions) -> Result<BuildOutput, CodegenError>;
}

/// The planned LLVM backend. Not implemented yet.
#[derive(Clone, Copy, Debug, Default)]
pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn name(&self) -> &'static str {
        "LLVM"
    }

    fn build(&self, module: &Module, _options: &BuildOptions) -> Result<BuildOutput, CodegenError> {
        brim_ir::validate(module).map_err(CodegenError::InvalidModule)?;
        Err(CodegenError::BackendUnavailable {
            backend: self.name(),
            reason: "native code generation has not been implemented yet".to_string(),
        })
    }
}

/// The backend used for native builds.
pub fn native_backend() -> Box<dyn Backend> {
    Box::new(LlvmBackend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_ir::{Block, FuncId, Function, Terminator, Type};

    fn empty_main() -> Module {
        Module {
            functions: vec![Function {
                name: "main".into(),
                param_count: 0,
                ret: Type::Void,
                slots: vec![],
                values: vec![],
                blocks: vec![Block {
                    insts: vec![],
                    term: Terminator::Return(None),
                }],
            }],
            entry: FuncId(0),
        }
    }

    fn options() -> BuildOptions {
        BuildOptions {
            output: PathBuf::from("out"),
            opt_level: OptLevel::Debug,
        }
    }

    #[test]
    fn llvm_backend_reports_unavailable() {
        let err = native_backend()
            .build(&empty_main(), &options())
            .unwrap_err();
        assert!(matches!(
            err,
            CodegenError::BackendUnavailable {
                backend: "LLVM",
                ..
            }
        ));
        assert_eq!(
            err.to_string(),
            "the LLVM backend is not available: native code generation has not been implemented yet"
        );
    }

    #[test]
    fn invalid_modules_are_rejected_first() {
        let mut module = empty_main();
        module.entry = FuncId(3);
        let err = LlvmBackend.build(&module, &options()).unwrap_err();
        assert!(matches!(err, CodegenError::InvalidModule(_)));
    }
}
