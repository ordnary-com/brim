//! Snapshot tests over the `.brim` programs in the repository's `tests/` and
//! `examples/` directories.
//!
//! - `tests/lexer`, `tests/parser`, `tests/diagnostics`: run `brim check`,
//!   compare stderr with the `.stderr` file next to the program.
//! - `tests/integration`: run `brim run`, compare stdout with `.stdout` and
//!   stderr with `.stderr` (an absent file means "must be empty").
//! - `examples`: every example must run successfully.
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
        let mut ctx = Context {
            cwd: workspace_root(),
            stdout: &mut stdout,
            stderr: &mut stderr,
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
