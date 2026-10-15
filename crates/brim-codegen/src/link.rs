//! Turning the LLVM module into an executable.
//!
//! The module is written as an object file, and the system C compiler links
//! it together with the Brim runtime (`runtime/brim_rt.c`, embedded in the
//! compiler). The C compiler is `$BRIM_CC`, else `$CC`, else `cc`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use inkwell::module::Module;
use inkwell::targets::{FileType, TargetMachine};

use crate::{BuildOptions, BuildOutput, CodegenError, OptLevel};

/// The C source of the Brim runtime.
pub(crate) const RUNTIME_SOURCE: &str = include_str!("../runtime/brim_rt.c");

/// HTTPS through libcurl; linked only into programs that use it.
const HTTPS_SOURCE: &str = include_str!("../runtime/brim_rt_https.c");

pub(crate) fn link(
    machine: &TargetMachine,
    module: &Module<'_>,
    options: &BuildOptions,
) -> Result<BuildOutput, CodegenError> {
    let dir = ScratchDir::create()
        .map_err(|e| CodegenError::Llvm(format!("cannot create a temporary directory: {e}")))?;
    let object = dir.path.join("program.o");
    machine
        .write_to_file(module, FileType::Object, &object)
        .map_err(|e| CodegenError::Llvm(e.to_string()))?;
    let runtime = dir.path.join("brim_rt.c");
    fs::write(&runtime, RUNTIME_SOURCE)
        .map_err(|e| CodegenError::Llvm(format!("cannot write the runtime source: {e}")))?;

    let linker = linker();
    let mut command = Command::new(&linker);
    command
        .arg("-std=c11")
        .arg(match options.opt_level {
            OptLevel::Debug => "-O0",
            OptLevel::Release => "-O2",
        })
        .arg(&object)
        .arg(&runtime);
    if module
        .get_function(brim_runtime::RuntimeFn::HttpRequest.symbol())
        .is_some()
    {
        let https = dir.path.join("brim_rt_https.c");
        fs::write(&https, HTTPS_SOURCE)
            .map_err(|e| CodegenError::Llvm(format!("cannot write the runtime source: {e}")))?;
        command.arg(&https).arg("-lcurl");
    }
    // The math functions (`sqrt`, `pow`, ...) are in libm on Linux.
    command.arg("-lm");
    command.arg("-o").arg(&options.output);
    let shown = format!("{command:?}").replace('"', "");
    let output = command.output().map_err(|e| CodegenError::LinkerNotFound {
        linker: linker.clone(),
        message: e.to_string(),
    })?;
    if !output.status.success() {
        return Err(CodegenError::LinkFailed {
            command: shown,
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(BuildOutput {
        artifacts: vec![options.output.clone()],
    })
}

fn linker() -> String {
    ["BRIM_CC", "CC"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "cc".to_string())
}

/// A temporary directory, removed when dropped.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn create() -> std::io::Result<ScratchDir> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("brim-build-{}-{n}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(ScratchDir { path })
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
