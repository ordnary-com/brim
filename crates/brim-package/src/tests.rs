use super::*;
use std::fs;
use std::process::Command;

/// A fresh directory for one test, which is also the working directory.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("brim-package-tests-{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_package(dir: &Path, name: &str, deps: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("brim.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\n{deps}"
        ),
    )
    .unwrap();
}

fn resolve_in(cwd: &Path, dir: &str, update: bool) -> Result<(Graph, Report), Failure> {
    let path = Path::new(dir).join("brim.toml");
    let text = fs::read_to_string(cwd.join(&path)).unwrap();
    let file = SourceFile::new(path.display().to_string(), text);
    let manifest = brim_manifest::parse(file.text()).unwrap();
    resolve(
        Path::new(dir),
        manifest,
        file,
        &Options {
            cwd: cwd.to_path_buf(),
            home: cwd.join("home"),
            update,
        },
    )
}

#[test]
fn path_dependencies() {
    let cwd = scratch("path");
    write_package(
        &cwd.join("app"),
        "app",
        "geometry = { path = \"../libs/geometry\" }\nutil = { path = \"../libs/util\" }\n",
    );
    write_package(
        &cwd.join("libs/geometry"),
        "geometry",
        "util = { path = \"../util\" }\n",
    );
    write_package(&cwd.join("libs/util"), "util", "");
    let (graph, report) = resolve_in(&cwd, "app", false).unwrap();
    let names: Vec<&str> = graph.packages.iter().map(Package::name).collect();
    assert_eq!(names, ["app", "geometry", "util"]);
    assert_eq!(graph.packages[1].dir, PathBuf::from("libs/geometry"));
    assert_eq!(graph.packages[0].dependency("util"), Some(2));
    assert_eq!(graph.packages[1].dependency("util"), Some(2));
    assert_eq!(
        graph.packages[1].lib_dir(),
        PathBuf::from("libs/geometry/src")
    );
    assert!(report.lock_written);
    let lock = fs::read_to_string(cwd.join("app/brim.lock")).unwrap();
    assert_eq!(
        lock,
        "# This file is written by brim. Do not edit it.\nversion = 1\n\n\
         [[package]]\nname = \"geometry\"\nversion = \"0.1.0\"\nsource = \"path+../libs/geometry\"\n\n\
         [[package]]\nname = \"util\"\nversion = \"0.1.0\"\nsource = \"path+../libs/util\"\n"
    );
    let (_, report) = resolve_in(&cwd, "app", false).unwrap();
    assert!(!report.lock_written);
}

#[test]
fn dependency_problems() {
    let cwd = scratch("problems");
    write_package(
        &cwd.join("app"),
        "app",
        "missing = { path = \"../missing\" }\n",
    );
    let failure = resolve_in(&cwd, "app", false).unwrap_err();
    assert_eq!(failure.diagnostic.message, "cannot find package `missing`");
    assert_eq!(failure.file.unwrap().name(), "app/brim.toml");

    write_package(&cwd.join("app"), "app", "other = { path = \"../lib\" }\n");
    write_package(&cwd.join("lib"), "lib", "");
    let failure = resolve_in(&cwd, "app", false).unwrap_err();
    assert_eq!(
        failure.diagnostic.message,
        "dependency `other` is the package `lib`"
    );

    write_package(
        &cwd.join("app"),
        "app",
        "lib = { path = \"../lib\" }\nb = { path = \"../b\" }\n",
    );
    write_package(&cwd.join("b"), "b", "lib = { path = \"../lib2\" }\n");
    write_package(&cwd.join("lib2"), "lib", "");
    let failure = resolve_in(&cwd, "app", false).unwrap_err();
    assert_eq!(
        failure.diagnostic.message,
        "two different packages are named `lib`"
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_lib(repo: &Path, text: &str) -> String {
    fs::write(repo.join("src/lib.brim"), text).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "change"]);
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn git_dependencies_are_locked() {
    let cwd = scratch("git");
    let repo = cwd.join("repo");
    write_package(&repo, "lib", "");
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let first = commit_lib(&repo, "pub fn version() -> i64 {\n    return 1\n}\n");
    git(&repo, &["tag", "v1"]);
    let shown = repo.display().to_string().replace('\\', "/");
    let url = if shown.starts_with('/') {
        format!("file://{shown}")
    } else {
        format!("file:///{shown}")
    };
    write_package(
        &cwd.join("app"),
        "app",
        &format!("lib = {{ git = \"{url}\" }}\n"),
    );

    let (graph, report) = resolve_in(&cwd, "app", false).unwrap();
    assert_eq!(graph.packages[1].git.as_ref().unwrap().1, first);
    assert!(
        fs::read_to_string(graph.packages[1].lib_file())
            .unwrap()
            .contains("return 1")
    );
    assert_eq!(report.fetched, std::slice::from_ref(&url));
    let lock = fs::read_to_string(cwd.join("app/brim.lock")).unwrap();
    assert!(lock.contains(&format!("commit = \"{first}\"")), "{lock}");

    // A new commit is not used until `brim update`.
    let second = commit_lib(&repo, "pub fn version() -> i64 {\n    return 2\n}\n");
    let (graph, report) = resolve_in(&cwd, "app", false).unwrap();
    assert_eq!(graph.packages[1].git.as_ref().unwrap().1, first);
    assert!(report.fetched.is_empty());
    let (graph, report) = resolve_in(&cwd, "app", true).unwrap();
    assert_eq!(graph.packages[1].git.as_ref().unwrap().1, second);
    assert!(report.lock_written);

    // A tag selects its commit.
    write_package(
        &cwd.join("app"),
        "app",
        &format!("lib = {{ git = \"{url}\", tag = \"v1\" }}\n"),
    );
    let (graph, _) = resolve_in(&cwd, "app", false).unwrap();
    assert_eq!(graph.packages[1].git.as_ref().unwrap().1, first);

    write_package(
        &cwd.join("app"),
        "app",
        &format!("lib = {{ git = \"{url}\", tag = \"v9\" }}\n"),
    );
    let failure = resolve_in(&cwd, "app", false).unwrap_err();
    assert_eq!(failure.diagnostic.message, "cannot get dependency `lib`");
    assert_eq!(failure.diagnostic.notes, ["the repository has no tag v9"]);
}

#[test]
fn normalizes_paths() {
    assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    assert_eq!(normalize(Path::new("app/../lib")), PathBuf::from("lib"));
    assert_eq!(normalize(Path::new("../lib")), PathBuf::from("../lib"));
    assert_eq!(normalize(Path::new("a/..")), PathBuf::from("."));
}
