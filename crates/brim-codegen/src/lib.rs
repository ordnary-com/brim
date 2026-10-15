//! The interface between the Brim compiler and native code generators.
//!
//! A [`Backend`] turns a validated IR [`Module`] into build artifacts on disk.
//! The frontend never talks to LLVM (or any other code generator) directly;
//! it only sees this trait. That keeps LLVM-specific code isolated in one
//! place and means the frontend, its tests and CI never require an LLVM
//! installation.
//!
//! # Backends
//!
//! [`LlvmBackend`] compiles IR to a native executable with LLVM 22 (through
//! the `inkwell` crate) and links it with the Brim runtime using the system C
//! compiler. It is only compiled in with the non-default `llvm` Cargo feature;
//! without it, [`LlvmBackend::build`] validates the module and returns
//! [`CodegenError::BackendUnavailable`]. See `docs/architecture.md` for how
//! IR maps onto LLVM and how run-time errors are reported.

#[cfg(feature = "llvm")]
mod debug;
#[cfg(feature = "llvm")]
mod link;
#[cfg(feature = "llvm")]
mod llvm;
mod trap;

use std::fmt;
use std::path::PathBuf;

use brim_diagnostics::SourceMap;
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
    /// The program's source files. Run-time error messages in the executable
    /// quote them, exactly like `brim run` does.
    pub source: SourceMap,
    /// Where compiled parts of the runtime are kept between builds (for
    /// example `~/.brim/cache`); `None` compiles them every time.
    pub cache_dir: Option<PathBuf>,
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
    /// LLVM rejected the generated code or failed to emit it; this is a
    /// compiler bug.
    Llvm(String),
    /// No usable C compiler was found to link the executable.
    LinkerNotFound { linker: String, message: String },
    /// The linker ran but failed.
    LinkFailed { command: String, output: String },
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
            CodegenError::Llvm(message) => {
                write!(f, "internal compiler error: LLVM: {message}")
            }
            CodegenError::LinkerNotFound { linker, message } => {
                write!(f, "cannot run the linker `{linker}`: {message}")
            }
            CodegenError::LinkFailed { command, output } => {
                write!(f, "linking failed: `{command}`")?;
                if !output.is_empty() {
                    write!(f, "\n{}", output.trim_end())?;
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

/// The LLVM backend. Functional only with the `llvm` Cargo feature.
#[derive(Clone, Copy, Debug, Default)]
pub struct LlvmBackend;

impl LlvmBackend {
    /// Whether this build of the compiler includes the LLVM backend.
    pub const AVAILABLE: bool = cfg!(feature = "llvm");
}

impl Backend for LlvmBackend {
    fn name(&self) -> &'static str {
        "LLVM"
    }

    fn build(&self, module: &Module, options: &BuildOptions) -> Result<BuildOutput, CodegenError> {
        brim_ir::validate(module).map_err(CodegenError::InvalidModule)?;
        #[cfg(feature = "llvm")]
        {
            llvm::build(module, options)
        }
        #[cfg(not(feature = "llvm"))]
        {
            let _ = options;
            Err(CodegenError::BackendUnavailable {
                backend: self.name(),
                reason: "this `brim` was compiled without the `llvm` feature".to_string(),
            })
        }
    }
}

/// Compiles `module` to textual LLVM IR, for inspection and tests.
#[cfg(feature = "llvm")]
pub fn emit_llvm_ir(module: &Module, source: &SourceMap) -> Result<String, CodegenError> {
    brim_ir::validate(module).map_err(CodegenError::InvalidModule)?;
    llvm::emit_ir(module, source)
}

/// The backend used for native builds.
pub fn native_backend() -> Box<dyn Backend> {
    Box::new(LlvmBackend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_diagnostics::SourceFile;
    use brim_ir::{Block, FuncId, Function, Terminator, Type};

    fn empty_main() -> Module {
        Module {
            structs: vec![],
            lists: vec![],
            maps: vec![],
            functions: vec![Function {
                consumes_self: false,
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
            source: {
                let mut map = SourceMap::new();
                map.add(SourceFile::new("main.brim", "fn main() {}\n"));
                map
            },
            cache_dir: None,
        }
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn llvm_backend_reports_unavailable_without_the_feature() {
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
            "the LLVM backend is not available: this `brim` was compiled without the `llvm` feature"
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
