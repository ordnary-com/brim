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
    assert!(
        stdout(&out).starts_with(&format!("brim {} (", env!("CARGO_PKG_VERSION"))),
        "{}",
        stdout(&out)
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

fn brim_with_home(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brim"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("BRIM_HOME", home)
        .output()
        .expect("brim runs")
}

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\n{deps}"
    )
}

#[test]
fn path_dependencies() {
    let dir = scratch("path_deps");
    let home = dir.join("home");
    fs::create_dir_all(dir.join("app/src")).unwrap();
    fs::create_dir_all(dir.join("geometry/src")).unwrap();
    fs::write(
        dir.join("app/brim.toml"),
        manifest("app", "geometry = { path = \"../geometry\" }\n"),
    )
    .unwrap();
    fs::write(
        dir.join("app/src/main.brim"),
        "import geometry\nimport geometry.shapes.{square}\n\nfn main() {\n    print(geometry.area(square(3)))\n}\n\nfn test_app() {\n    assert(true)\n}\n",
    )
    .unwrap();
    fs::write(dir.join("geometry/brim.toml"), manifest("geometry", "")).unwrap();
    fs::write(
        dir.join("geometry/src/lib.brim"),
        "import shapes.{Square}\n\npub fn area(s: Square) -> i64 {\n    return s.side * s.side\n}\n\nfn test_area() {\n    assert(false)\n}\n",
    )
    .unwrap();
    fs::write(
        dir.join("geometry/src/shapes.brim"),
        "pub struct Square {\n    pub side: i64,\n}\n\npub fn square(side: i64) -> Square {\n    return Square { side: side }\n}\n",
    )
    .unwrap();

    let app = dir.join("app");
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stdout(&out), "9\n", "{}", stderr(&out));
    assert_eq!(stderr(&out), "updated brim.lock\n");
    let lock = fs::read_to_string(app.join("brim.lock")).unwrap();
    assert!(lock.contains("source = \"path+../geometry\""), "{lock}");
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stderr(&out), "");

    // Only the project's own tests run.
    let out = brim_with_home(&app, &home, &["test"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(stdout(&out).contains("running 1 test\n"));
    // A library is tested on its own.
    let out = brim_with_home(&dir.join("geometry"), &home, &["test"]);
    assert_eq!(out.status.code(), Some(101));
    assert!(stdout(&out).contains("test test_area ... FAILED"));

    // A module of a dependency can be imported on its own.
    fs::write(
        dir.join("app/src/main.brim"),
        "import geometry.shapes\n\nfn main() {\n    let s: shapes.Square = shapes.square(1)\n    print(s.side)\n}\n",
    )
    .unwrap();
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stdout(&out), "1\n", "{}", stderr(&out));

    fs::write(
        dir.join("app/brim.toml"),
        manifest("app", "geometry = { path = \"../nowhere\" }\n"),
    )
    .unwrap();
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr(&out).contains("error[M0008]: cannot find package `geometry`"),
        "{}",
        stderr(&out)
    );
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn git_dependencies_and_update() {
    let dir = scratch("git_deps");
    let home = dir.join("home");
    let repo = dir.join("greet");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("brim.toml"), manifest("greet", "")).unwrap();
    let publish = |text: &str| {
        fs::write(
            repo.join("src/lib.brim"),
            format!("pub fn hello() -> string {{\n    return \"{text}\"\n}}\n"),
        )
        .unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", text]);
    };
    git(&repo, &["init", "--quiet"]);
    publish("hello v1");

    let shown = repo.display().to_string().replace('\\', "/");
    let url = if shown.starts_with('/') {
        format!("file://{shown}")
    } else {
        format!("file:///{shown}")
    };
    let app = dir.join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("brim.toml"),
        manifest("app", &format!("greet = {{ git = \"{url}\" }}\n")),
    )
    .unwrap();
    fs::write(
        app.join("src/main.brim"),
        "import greet\n\nfn main() {\n    print(greet.hello())\n}\n",
    )
    .unwrap();

    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stdout(&out), "hello v1\n", "{}", stderr(&out));
    assert_eq!(stderr(&out), format!("fetched {url}\nupdated brim.lock\n"));
    assert!(
        fs::read_to_string(app.join("brim.lock"))
            .unwrap()
            .contains("commit = \"")
    );

    // The lock file keeps the commit until `brim update`.
    publish("hello v2");
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stdout(&out), "hello v1\n");
    assert_eq!(stderr(&out), "");
    let out = brim_with_home(&app, &home, &["update"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).ends_with("ok: 1 dependency up to date\n"),
        "{}",
        stderr(&out)
    );
    let out = brim_with_home(&app, &home, &["run"]);
    assert_eq!(stdout(&out), "hello v2\n");
}

#[test]
fn gen_ts_matches_the_example() {
    let out = brim(&["gen", "ts", "examples/api.brim"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let expected = fs::read_to_string(root().join("examples/api.ts")).unwrap();
    assert_eq!(
        stdout(&out),
        expected.replace("\r\n", "\n"),
        "regenerate with `brim gen ts examples/api.brim -o examples/api.ts`"
    );

    let dir = scratch("gen_ts");
    fs::write(
        dir.join("a.brim"),
        "import b\n\npub struct Item {\n    x: i64,\n}\n\npub struct Pair {\n    first: b.Item,\n}\n",
    )
    .unwrap();
    fs::write(dir.join("b.brim"), "pub struct Item {\n    y: i64,\n}\n").unwrap();
    let out = brim_in(&dir, &["gen", "ts", "a.brim", "-o", "out/api.ts"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out)
            .contains("error[G0001]: two exported types are named `Item`: `Item` and `b.Item`"),
        "{}",
        stderr(&out)
    );
}

const HTTP_SERVER: &str = r#"import std.http

fn main() {
    match serve() {
        Ok(_) => print("stopped")
        Err(e) => print("error: " + e)
    }
}

fn serve() -> Result[i64, string] {
    let server: http.Server = http.serve("127.0.0.1", 0)?
    print(server.port())
    while true {
        let request: http.Request = server.next()?
        if request.path == "/stop" {
            request.respond(http.text(200, "bye"))?
            break
        }
        let name: string = match_name(request)
        request.respond(http.json(200, "{\"hello\": \"" + name + "\", \"body\": \"" + request.body + "\"}"))?
    }
    server.close()
    return Ok(0)
}

fn match_name(request: http.Request) -> string {
    match request.param("name") {
        Some(name) => return name
        None => return "nobody"
    }
}
"#;

const HTTP_CLIENT: &str = r#"import std.http

fn main() {
    let base: string = "http://127.0.0.1:" + args()[0]
    match talk(base) {
        Ok(_) => print("done")
        Err(e) => print("error: " + e)
    }
}

fn talk(base: string) -> Result[i64, string] {
    let a: http.Response = http.get(base + "/greet?name=" + http.encode("Ada L"))?
    print(to_string(a.status) + " " + a.body)
    let b: http.Response = http.post(base + "/greet", "text/plain", "data")?
    match b.header("Content-Type") {
        Some(t) => print(t + " " + b.body)
        None => print("no content type")
    }
    let c: http.Response = http.get(base + "/stop")?
    print(c.body)
    return Ok(0)
}
"#;

#[test]
fn http_server_and_client() {
    use std::io::BufRead as _;
    let dir = scratch("http");
    fs::write(dir.join("server.brim"), HTTP_SERVER).unwrap();
    fs::write(dir.join("client.brim"), HTTP_CLIENT).unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_brim"))
        .args(["run", "server.brim"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(server.stdout.take().unwrap()).lines();
    let port = lines.next().unwrap().unwrap();
    let client = brim_in(&dir, &["run", "client.brim", &port]);
    assert_eq!(
        stdout(&client),
        "200 {\"hello\": \"Ada L\", \"body\": \"\"}\napplication/json {\"hello\": \"nobody\", \"body\": \"data\"}\nbye\ndone\n",
        "{}",
        stderr(&client)
    );
    assert_eq!(lines.next().unwrap().unwrap(), "stopped");
    assert!(server.wait().unwrap().success());
}
