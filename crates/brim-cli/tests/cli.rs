//! End-to-end tests of the `brim` executable: argument handling, exit codes
//! and project manifests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn brim_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brim"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("brim runs")
}

fn brim(args: &[&str]) -> Output {
    brim_in(&root(), args)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A fresh, empty directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn version_and_help() {
    let out = brim(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out),
        format!("brim {}\n", env!("CARGO_PKG_VERSION"))
    );

    let out = brim(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).starts_with("Brim: the Ordnary programming language"));
    assert!(stdout(&out).contains("check [PATH]"));

    let out = brim(&["help", "check"]);
    assert!(stdout(&out).contains("--emit <tokens|ast|ir>"));
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        &[][..],
        &["frobnicate"],
        &["check", "a.brim", "b.brim"],
        &["check", "--emit=x"],
    ] {
        let out = brim(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).starts_with("error: "), "{args:?}");
        assert!(stderr(&out).contains("--help"), "{args:?}");
    }
}

#[test]
fn check_and_run_examples() {
    let out = brim(&["check", "examples/hello.brim"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stderr(&out), "ok: no errors found in examples/hello.brim\n");

    let out = brim(&["run", "examples/hello.brim"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "Hello from Ordnary\nBrim version: 0.1.0\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn compile_errors_exit_1() {
    let out = brim(&["check", "tests/diagnostics/undefined_variable.brim"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("error[E0200]: undefined variable `username`"));
    assert!(stderr(&out).contains(" --> tests/diagnostics/undefined_variable.brim:4:11"));
    assert_eq!(stdout(&out), "");
}

#[test]
fn input_errors_exit_3() {
    let out = brim(&["check", "does/not/exist.brim"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        stderr(&out),
        "error: file not found\n --> does/not/exist.brim\n"
    );

    let out = brim(&["check", "Cargo.toml"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("not a Brim source file"));

    let dir = scratch("invalid_utf8");
    fs::write(dir.join("bad.brim"), b"fn main() {\n  print(\"\xff\")\n}\n").unwrap();
    let out = brim_in(&dir, &["check", "bad.brim"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("file is not valid UTF-8"));
    assert!(stderr(&out).contains("bad.brim:2:10"), "{}", stderr(&out));
}

#[test]
fn runtime_errors_exit_101() {
    let out = brim(&["run", "tests/integration/overflow.brim"]);
    assert_eq!(out.status.code(), Some(101));
    assert_eq!(stdout(&out), "start\n");
    assert!(stderr(&out).starts_with("error[R0001]: integer overflow in `*`"));
}

#[cfg(not(feature = "llvm"))]
#[test]
fn build_reports_unavailable_backend() {
    let out = brim(&["build", "examples/hello.brim"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(stderr(&out).contains("the LLVM backend is not available"));
    assert!(stderr(&out).contains("brim run examples/hello.brim"));
    // Programs with errors still fail with compile errors first.
    let out = brim(&["build", "tests/diagnostics/missing_main.brim"]);
    assert_eq!(out.status.code(), Some(1));
}

#[cfg(feature = "llvm")]
#[test]
fn build_writes_an_executable() {
    let dir = scratch("build_single_file");
    fs::copy(root().join("examples/hello.brim"), dir.join("hello.brim")).unwrap();
    let out = brim_in(&dir, &["build", "hello.brim"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let exe = format!("hello{}", std::env::consts::EXE_SUFFIX);
    assert_eq!(stderr(&out), format!("built {exe}\n"));
    let run = Command::new(dir.join(&exe)).output().unwrap();
    assert_eq!(run.status.code(), Some(0));
    assert!(stdout(&run).starts_with("Hello from Ordnary\n"));

    // `-o` creates missing directories.
    let out = brim_in(&dir, &["build", "--release", "-o", "out/app", "hello.brim"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(dir.join("out").is_dir());

    // Programs with errors still fail with compile errors first.
    let out = brim(&["build", "tests/diagnostics/missing_main.brim"]);
    assert_eq!(out.status.code(), Some(1));
}

#[cfg(feature = "llvm")]
#[test]
fn build_project_into_target() {
    let dir = scratch("build_project");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"greeter\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.brim"),
        "fn main() {\n    print(\"hi\")\n}\n",
    )
    .unwrap();
    let out = brim_in(&dir, &["build"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let exe = dir
        .join("target")
        .join(format!("greeter{}", std::env::consts::EXE_SUFFIX));
    let run = Command::new(exe).output().unwrap();
    assert_eq!(stdout(&run), "hi\n");
}

#[cfg(feature = "llvm")]
#[test]
fn build_reports_a_missing_linker() {
    let dir = scratch("build_missing_linker");
    fs::copy(root().join("examples/hello.brim"), dir.join("hello.brim")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_brim"))
        .args(["build", "hello.brim"])
        .current_dir(&dir)
        .env("BRIM_CC", "brim-no-such-compiler")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(stderr(&out).contains("cannot run the linker `brim-no-such-compiler`"));
    assert!(stderr(&out).contains("set `BRIM_CC`"));
}

#[test]
fn emit_intermediate_forms() {
    let out = brim(&["check", "--emit", "ir", "examples/functions.brim"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).starts_with("; entry: main\n"));
    assert!(stdout(&out).contains("fn add(s0 a: i64, s1 b: i64) -> i64 {"));

    let out = brim(&["check", "--emit=tokens", "examples/hello.brim"]);
    assert!(stdout(&out).contains("2:1\tFn"));

    let out = brim(&["check", "--emit=ast", "examples/hello.brim"]);
    assert!(stdout(&out).contains("FnDecl"));
}

#[test]
fn projects_with_manifests() {
    let dir = scratch("project_ok");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/main.brim"), "fn main() { print(\"demo\") }\n").unwrap();

    // Found from the current directory, from a subdirectory, and by path.
    assert_eq!(stdout(&brim_in(&dir, &["run"])), "demo\n");
    assert_eq!(stdout(&brim_in(&dir.join("src"), &["run"])), "demo\n");
    assert_eq!(stdout(&brim_in(&dir, &["run", "."])), "demo\n");
    assert_eq!(stdout(&brim_in(&dir, &["run", "brim.toml"])), "demo\n");

    let out = brim_in(&dir, &["check"]);
    assert_eq!(stderr(&out), "ok: no errors found in src/main.brim\n");
}

#[test]
fn manifest_errors() {
    let dir = scratch("project_bad");
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"Bad Name\"\nversion = \"1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let out = brim_in(&dir, &["check"]);
    assert_eq!(out.status.code(), Some(3));
    let err = stderr(&out);
    assert!(
        err.contains("error[M0002]: invalid package name `Bad Name`"),
        "{err}"
    );
    assert!(err.contains(" --> brim.toml:2:8"), "{err}");
    assert!(
        err.contains("error[M0003]: invalid package version `1.0`"),
        "{err}"
    );

    let dir = scratch("project_missing_entry");
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let out = brim_in(&dir, &["run"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("file not found\n --> src/main.brim"));
    assert!(stderr(&out).contains("`[build] entry`"));

    let dir = scratch("no_project");
    let out = brim_in(&dir, &["run"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("no brim.toml found"));
}

/// A program that uses its arguments, standard input and a file.
const IO_PROGRAM: &str = r#"fn main() {
    let words: List[string] = args()
    let mut lines: List[string] = []
    while true {
        match read_line() {
            Some(line) => lines.push(line.to_upper())
            None => break
        }
    }
    match write_file("out.txt", words.join("+") + "\n" + lines.join("\n")) {
        Ok(n) => print("wrote " + to_string(n) + " bytes")
        Err(message) => print(message)
    }
    match read_file("out.txt") {
        Ok(text) => print(text)
        Err(message) => print(message)
    }
    match write_file("missing/dir/out.txt", "x") {
        Ok(n) => print(n)
        Err(message) => print(message)
    }
    exit(words.len())
}
"#;

const IO_EXPECTED: &str =
    "wrote 13 bytes\na+b+c\nONE\nTWO\ncannot write file \"missing/dir/out.txt\": file not found\n";

fn run_with_stdin(mut command: Command, input: &str) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn run_passes_arguments_and_input() {
    let dir = scratch("run_io");
    fs::write(dir.join("io.brim"), IO_PROGRAM).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_brim"));
    command
        .args(["run", "io.brim", "a+b", "c"])
        .current_dir(&dir);
    let out = run_with_stdin(command, "one\ntwo\n");
    assert_eq!(stdout(&out), IO_EXPECTED, "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(2));
}

#[cfg(feature = "llvm")]
#[test]
fn native_programs_get_arguments_and_input() {
    let dir = scratch("native_io");
    fs::write(dir.join("io.brim"), IO_PROGRAM).unwrap();
    let built = brim_in(&dir, &["build", "io.brim", "-o", "io"]);
    assert_eq!(built.status.code(), Some(0), "{}", stderr(&built));
    let mut command = Command::new(dir.join("io"));
    command
        .args(["a+b", "c"])
        .current_dir(&dir)
        .env("BRIM_RT_CHECK", "1");
    let out = run_with_stdin(command, "one\ntwo\n");
    assert_eq!(stdout(&out), IO_EXPECTED, "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn new_creates_a_project_that_builds_and_tests() {
    let dir = scratch("new_project");
    let out = brim_in(&dir, &["new", "hello"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "created project `hello` in hello\n");
    let project = dir.join("hello");
    assert!(project.join(".gitignore").is_file());
    let manifest = fs::read_to_string(project.join("brim.toml")).unwrap();
    assert!(manifest.contains("name = \"hello\""), "{manifest}");

    let out = brim_in(&project, &["run"]);
    assert_eq!(stdout(&out), "Hello, world!\n", "{}", stderr(&out));
    let out = brim_in(&project, &["test"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("test test_greeting ... ok"));

    let out = brim_in(&dir, &["new", "hello"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("already exists"));
    let out = brim_in(&dir, &["new", "Hello"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("`Hello` cannot be a package name"));
}

const TESTED_PROGRAM: &str = r#"import shapes

fn double(n: i64) -> i64 {
    return n * 2
}

fn test_double() {
    assert_eq(double(21), 42)
}

fn test_wrong() {
    print("about to fail")
    assert_eq(double(2), 5)
}

fn test_exit() {
    exit(3)
}

fn helper_not_a_test() {
    assert(false)
}
"#;

#[test]
fn test_runs_test_functions() {
    let dir = scratch("test_command");
    fs::write(dir.join("lib.brim"), TESTED_PROGRAM).unwrap();
    fs::write(
        dir.join("shapes.brim"),
        "pub fn area(w: i64, h: i64) -> i64 {\n    return w * h\n}\n\nfn test_area() {\n    assert_eq(area(2, 3), 6)\n}\n",
    )
    .unwrap();

    let out = brim_in(&dir, &["test", "lib.brim"]);
    assert_eq!(out.status.code(), Some(101), "{}", stderr(&out));
    let text = stdout(&out);
    for line in [
        "running 4 tests\n",
        "test test_double ... ok\n",
        "test test_wrong ... FAILED\n",
        "test test_exit ... FAILED\n",
        "test shapes.test_area ... ok\n",
        "---- test_wrong ----\nabout to fail\n",
        "test result: FAILED. 2 passed; 2 failed\n",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert!(!text.contains("helper_not_a_test"));
    let errors = stderr(&out);
    assert!(errors.contains("note: left:  4\n"), "{errors}");
    assert!(errors.contains("the test called `exit(3)`"), "{errors}");

    let out = brim_in(&dir, &["test", "lib.brim", "double"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("running 1 test\n"));
    assert!(stdout(&out).contains("test result: ok. 1 passed; 0 failed\n"));

    // A test cannot take parameters.
    fs::write(dir.join("bad.brim"), "fn test_it(n: i64) {\n}\n").unwrap();
    let out = brim_in(&dir, &["test", "bad.brim"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("error[E0244]"));
    // Outside `brim test`, `main` is still required.
    let out = brim_in(&dir, &["check", "lib.brim"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("error[E0210]"));
}

#[test]
fn fmt_formats_files_in_place() {
    let dir = scratch("fmt_project");
    fs::create_dir_all(dir.join("src/util")).unwrap();
    fs::create_dir_all(dir.join("target")).unwrap();
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    let messy = "fn main(){\nprint( 1+2 )   // three\n}\n";
    let tidy = "fn main() {\n    print(1 + 2) // three\n}\n";
    fs::write(dir.join("src/main.brim"), messy).unwrap();
    fs::write(dir.join("src/util/ok.brim"), tidy).unwrap();
    fs::write(dir.join("target/skipped.brim"), messy).unwrap();

    let out = brim_in(&dir, &["fmt", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out).replace('\\', "/"), "src/main.brim\n");
    assert_eq!(
        fs::read_to_string(dir.join("src/main.brim")).unwrap(),
        messy
    );

    let out = brim_in(&dir, &["fmt"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(dir.join("src/main.brim")).unwrap(), tidy);
    assert_eq!(
        fs::read_to_string(dir.join("target/skipped.brim")).unwrap(),
        messy
    );
    let out = brim_in(&dir, &["fmt", "--check", "src"]);
    assert_eq!(out.status.code(), Some(0));

    fs::write(dir.join("bad.brim"), "fn main( {\n").unwrap();
    let out = brim_in(&dir, &["fmt", "bad.brim"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("error[E"), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(dir.join("bad.brim")).unwrap(),
        "fn main( {\n"
    );

    let out = run_with_stdin(
        {
            let mut c = Command::new(env!("CARGO_BIN_EXE_brim"));
            c.args(["fmt", "-"]).current_dir(&dir);
            c
        },
        messy,
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), tidy);
}
