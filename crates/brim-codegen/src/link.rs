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
    let linker = linker();
    let opt = match options.opt_level {
        OptLevel::Debug => "-O0",
        OptLevel::Release => "-O2",
    };
    let runtime = runtime_object(&linker, opt, "brim_rt", RUNTIME_SOURCE, &dir, options)?;
    let mut command = Command::new(&linker);
    command.arg(opt).arg(&object).arg(&runtime);
    if options.opt_level == OptLevel::Debug {
        command.arg("-g");
    }
    if module
        .get_function(brim_runtime::RuntimeFn::HttpRequest.symbol())
        .is_some()
    {
        let https = runtime_object(&linker, opt, "brim_rt_https", HTTPS_SOURCE, &dir, options)?;
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
    let mut artifacts = vec![options.output.clone()];
    // On macOS the debug information stays in the object file, which is
    // removed with the scratch directory: `dsymutil` collects it into
    // `program.dSYM` next to the executable, where lldb finds it.
    if cfg!(target_os = "macos") && options.opt_level == OptLevel::Debug {
        let ok = Command::new("dsymutil")
            .arg(&options.output)
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            let mut dsym = options.output.clone().into_os_string();
            dsym.push(".dSYM");
            artifacts.push(PathBuf::from(dsym));
        }
    }
    Ok(BuildOutput { artifacts })
}

/// The object file of a part of the runtime: from the cache when it was
/// compiled before with the same compiler and flags, else compiled now (and
/// kept in the cache).
fn runtime_object(
    linker: &str,
    opt: &str,
    name: &str,
    source: &str,
    dir: &ScratchDir,
    options: &BuildOptions,
) -> Result<PathBuf, CodegenError> {
    let key_text = format!("{linker}\n{opt}\n{}\n{source}", std::env::consts::ARCH);
    let key: String = brim_runtime::crypto::sha256(key_text.as_bytes())[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let cached = options
        .cache_dir
        .as_ref()
        .map(|d| d.join(format!("{name}-{key}.o")));
    if let Some(cached) = &cached
        && cached.is_file()
    {
        return Ok(cached.clone());
    }
    let c_file = dir.path.join(format!("{name}.c"));
    fs::write(&c_file, source)
        .map_err(|e| CodegenError::Llvm(format!("cannot write the runtime source: {e}")))?;
    let object = dir.path.join(format!("{name}.o"));
    let mut command = Command::new(linker);
    command
        .arg("-std=c11")
        .arg(opt)
        .arg("-c")
        .arg(&c_file)
        .arg("-o")
        .arg(&object);
    let shown = format!("{command:?}").replace('"', "");
    let output = command.output().map_err(|e| CodegenError::LinkerNotFound {
        linker: linker.to_string(),
        message: e.to_string(),
    })?;
    if !output.status.success() {
        return Err(CodegenError::LinkFailed {
            command: shown,
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    // Into the cache under a temporary name first, so another build never
    // sees half a file.
    if let Some(cached) = cached
        && let Some(cache) = cached.parent()
        && fs::create_dir_all(cache).is_ok()
    {
        let partial = cache.join(format!("{name}-{key}.{}.tmp", std::process::id()));
        if fs::copy(&object, &partial).is_ok() && fs::rename(&partial, &cached).is_ok() {
            return Ok(cached);
        }
        let _ = fs::remove_file(&partial);
    }
    Ok(object)
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
