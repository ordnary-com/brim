//! Snapshot tests over the `.brim` programs in the repository's `tests/` and
//! `examples/` directories.
//!
//! - `tests/lexer`, `tests/parser`, `tests/diagnostics`: run `brim check`,
//!   compare stderr with the `.stderr` file next to the program.
//! - `tests/integration`: run `brim run`, compare stdout with `.stdout` and
//!   stderr with `.stderr` (an absent file means "must be empty").
//! - `examples`: every example must run successfully.
//! - With the `llvm` feature, every integration program and example is also
//!   built as a native executable, which must produce exactly the same
//!   stdout, stderr and exit code as the interpreter (differential testing).
//!   The executables run with `BRIM_RT_CHECK=1`, so string leaks and use
//!   after free are failures too.
//!
//! Expected exit codes default to 1 for `check` suites and 0 for
//! `integration`; a first line `// exit: N` overrides that.
//!
//! Run with `BLESS=1` to (re)write the expected-output files after an
//! intentional change, then review the diff.

use std::fs;
use std::path::{Path, PathBuf};

use brim_cli::{Context, run};
use brim_diagnostics::ColorChoice;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists")
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

fn brim(args: &[&str]) -> Outcome {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = {
        let mut stdin = std::io::empty();
        let mut ctx = Context {
            cwd: workspace_root(),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin: &mut stdin,
            color: ColorChoice::Never,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run(&args, &mut ctx)
    };
    Outcome {
        code,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

fn programs(dir: &str) -> Vec<String> {
    let mut files: Vec<String> = fs::read_dir(workspace_root().join(dir))
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "brim"))
        .map(|p| format!("{dir}/{}", p.file_name().unwrap().to_string_lossy()))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no programs in {dir}");
    files
}

fn expected_exit(path: &str, default: i32) -> i32 {
    let text = fs::read_to_string(workspace_root().join(path)).unwrap();
    text.lines()
        .next()
        .and_then(|l| l.strip_prefix("// exit: "))
        .map_or(default, |n| n.trim().parse().expect("valid exit code"))
}

/// Compares `actual` with the snapshot file, or writes it when blessing.
/// Returns a description of the mismatch, if any.
fn compare(snapshot: &Path, actual: &str) -> Option<String> {
    let bless = std::env::var_os("BLESS").is_some();
    let expected = fs::read_to_string(snapshot).ok();
    if bless {
        if actual.is_empty() {
            let _ = fs::remove_file(snapshot);
        } else if expected.as_deref() != Some(actual) {
            fs::write(snapshot, actual).unwrap();
        }
        return None;
    }
    let expected = expected.unwrap_or_default();
    (expected != actual).then(|| {
        format!(
            "--- {} (expected)\n{expected}\n--- actual\n{actual}",
            snapshot.display()
        )
    })
}

fn run_suite(dir: &str, command: &str, default_exit: i32) {
    let mut failures = Vec::new();
    for program in programs(dir) {
        let out = brim(&[command, &program]);
        let expected = expected_exit(&program, default_exit);
        if out.code != expected {
            failures.push(format!(
                "{program}: exit code {} (expected {expected})\nstderr:\n{}",
                out.code, out.stderr
            ));
        }
        let base = workspace_root().join(&program);
        if let Some(diff) = compare(&base.with_extension("stderr"), &out.stderr) {
            failures.push(format!("{program}: stderr differs\n{diff}"));
        }
        if let Some(diff) = compare(&base.with_extension("stdout"), &out.stdout) {
            failures.push(format!("{program}: stdout differs\n{diff}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn lexer_errors() {
    run_suite("tests/lexer", "check", 1);
}

#[test]
fn parser_errors() {
    run_suite("tests/parser", "check", 1);
}

#[test]
fn semantic_diagnostics() {
    run_suite("tests/diagnostics", "check", 1);
}

#[test]
fn integration_programs() {
    run_suite("tests/integration", "run", 0);
}

/// The `main.brim` of every program in `tests/modules`, each a directory of
/// modules.
fn module_programs() -> Vec<String> {
    let mut dirs: Vec<String> = fs::read_dir(workspace_root().join("tests/modules"))
        .expect("tests/modules exists")
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.join("main.brim").is_file())
        .map(|p| {
            format!(
                "tests/modules/{}/main.brim",
                p.file_name().unwrap().to_string_lossy()
            )
        })
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn programs_with_modules() {
    let mut failures = Vec::new();
    for program in module_programs() {
        let out = brim(&["run", &program]);
        let expected = expected_exit(&program, 0);
        if out.code != expected {
            failures.push(format!(
                "{program}: exit code {} (expected {expected})\nstderr:\n{}",
                out.code, out.stderr
            ));
        }
        let base = workspace_root().join(&program);
        if let Some(diff) = compare(&base.with_extension("stderr"), &out.stderr) {
            failures.push(format!("{program}: stderr differs\n{diff}"));
        }
        if let Some(diff) = compare(&base.with_extension("stdout"), &out.stdout) {
            failures.push(format!("{program}: stdout differs\n{diff}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn examples_run() {
    for program in programs("examples") {
        let out = brim(&["run", &program]);
        assert_eq!(out.code, 0, "{program} failed:\n{}", out.stderr);
        assert_eq!(out.stderr, "", "{program} produced diagnostics");
        assert!(!out.stdout.is_empty(), "{program} printed nothing");
    }
    let out = brim(&["run", "examples/project"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "Hello from a Brim project\n");
}

/// Builds `program` natively with `brim build` and runs the executable.
#[cfg(feature = "llvm")]
fn run_native(program: &str, release: bool) -> Result<Outcome, String> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native");
    fs::create_dir_all(&dir).unwrap();
    let mode = if release { "release" } else { "debug" };
    let name = program.replace(['/', '.'], "_");
    let exe = dir.join(format!("{name}-{mode}{}", std::env::consts::EXE_SUFFIX));
    let exe_arg = exe.to_string_lossy().into_owned();
    let mut args = vec!["build", program, "-o", &exe_arg];
    if release {
        args.push("--release");
    }
    let built = brim(&args);
    if built.code != 0 {
        return Err(format!(
            "{program}: `brim build` failed with exit code {}\n{}",
            built.code, built.stderr
        ));
    }
    // Memory checking: leaks and use after free fail the comparison.
    let out = std::process::Command::new(&exe)
        .current_dir(workspace_root())
        .env("BRIM_RT_CHECK", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: cannot run {}: {e}", exe.display()))?;
    Ok(Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

#[cfg(feature = "llvm")]
fn native_suite(release: bool) {
    let mut failures = Vec::new();
    // Programs with compile errors cannot be built.
    let modules = module_programs()
        .into_iter()
        .filter(|p| expected_exit(p, 0) != 1);
    let programs = programs("tests/integration")
        .into_iter()
        .chain(programs("examples"))
        .chain(modules);
    for program in programs {
        let interpreted = brim(&["run", &program]);
        let native = match run_native(&program, release) {
            Ok(native) => native,
            Err(failure) => {
                failures.push(failure);
                continue;
            }
        };
        if native.code != interpreted.code {
            failures.push(format!(
                "{program}: native exit code {} (interpreter: {})\nstderr:\n{}",
                native.code, interpreted.code, native.stderr
            ));
        }
        for (stream, native, interpreted) in [
            ("stdout", &native.stdout, &interpreted.stdout),
            ("stderr", &native.stderr, &interpreted.stderr),
        ] {
            if native != interpreted {
                failures.push(format!(
                    "{program}: {stream} differs\n--- interpreter\n{interpreted}\n--- native\n{native}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[cfg(feature = "llvm")]
#[test]
fn native_matches_interpreter() {
    native_suite(false);
}

#[cfg(feature = "llvm")]
#[test]
fn native_release_matches_interpreter() {
    native_suite(true);
}
