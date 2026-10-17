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
        dir.join(".brimproj"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\n{deps}"
        ),
    )
    .unwrap();
}

fn resolve_in(cwd: &Path, dir: &str, update: bool) -> Result<(Graph, Report), Failure> {
    let path = Path::new(dir).join(".brimproj");
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
            registry: None,
            offline: false,
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
    let lock = fs::read_to_string(cwd.join("app/.brimlock")).unwrap();
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
    assert_eq!(
        failure.file.unwrap().name().replace('\\', "/"),
        "app/.brimproj"
    );

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
    let lock = fs::read_to_string(cwd.join("app/.brimlock")).unwrap();
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

/// A registry in a directory (see `registry.rs`), with packages added by
/// [`TestRegistry::add`].
struct TestRegistry {
    dir: PathBuf,
    index: std::collections::BTreeMap<String, Vec<serde_json::Value>>,
}

impl TestRegistry {
    fn new(dir: PathBuf) -> TestRegistry {
        TestRegistry {
            dir,
            index: Default::default(),
        }
    }

    fn url(&self) -> String {
        format!("file://{}", self.dir.display())
    }

    /// Adds `name` `version` with registry dependencies `deps`.
    fn add(&mut self, name: &str, version: &str, deps: &[(&str, &str)]) -> &mut TestRegistry {
        let deps_toml: String = deps
            .iter()
            .map(|(n, r)| format!("{n} = \"{r}\"\n"))
            .collect();
        let manifest = format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2026\"\n\n\
             [dependencies]\n{deps_toml}"
        );
        let lib = format!("pub fn version() -> string {{\n    return \"{version}\"\n}}\n");
        let archive = registry::pack(
            Path::new("."),
            &[],
            &[
                (".brimproj".into(), manifest.into_bytes()),
                ("src/lib.brim".into(), lib.into_bytes()),
            ],
            &format!("{name}-{version}"),
        )
        .unwrap();
        let download = self.dir.join(format!("api/v1/download/{name}/{version}"));
        fs::create_dir_all(download.parent().unwrap()).unwrap();
        fs::write(&download, &archive).unwrap();
        let checksum = registry::hex(&brim_runtime::crypto::sha256(&archive));
        let deps: serde_json::Map<String, serde_json::Value> = deps
            .iter()
            .map(|(n, r)| (n.to_string(), serde_json::Value::from(*r)))
            .collect();
        self.index
            .entry(name.into())
            .or_default()
            .push(serde_json::json!({
                "version": version,
                "checksum": checksum,
                "edition": "2026",
                "dependencies": deps,
                "yanked": false,
            }));
        self.write();
        self
    }

    fn yank(&mut self, name: &str, version: &str) {
        for v in self.index.get_mut(name).unwrap() {
            if v["version"] == version {
                v["yanked"] = true.into();
            }
        }
        self.write();
    }

    fn corrupt(&self, name: &str, version: &str) {
        let download = self.dir.join(format!("api/v1/download/{name}/{version}"));
        let mut bytes = fs::read(&download).unwrap();
        bytes.push(0);
        fs::write(download, bytes).unwrap();
    }

    fn write(&self) {
        let index = self.dir.join("api/v1/index");
        fs::create_dir_all(&index).unwrap();
        for (name, versions) in &self.index {
            let doc = serde_json::json!({ "name": name, "versions": versions });
            fs::write(index.join(name), doc.to_string()).unwrap();
        }
    }
}

fn resolve_with(cwd: &Path, registry: &str, update: bool) -> Result<(Graph, Report), Failure> {
    let path = Path::new("app").join(".brimproj");
    let text = fs::read_to_string(cwd.join(&path)).unwrap();
    let file = SourceFile::new(path.display().to_string(), text);
    let manifest = brim_manifest::parse(file.text()).unwrap();
    resolve(
        Path::new("app"),
        manifest,
        file,
        &Options {
            cwd: cwd.to_path_buf(),
            home: cwd.join("home"),
            update,
            registry: Some(registry.to_string()),
            offline: false,
        },
    )
}

fn versions(graph: &Graph) -> Vec<String> {
    let mut out: Vec<String> = graph.packages[1..]
        .iter()
        .map(|p| format!("{} {}", p.name(), p.manifest.package.version))
        .collect();
    out.sort();
    out
}

fn failure_text(failure: &Failure) -> String {
    let d = &failure.diagnostic;
    let mut out = format!("{}: {}", d.code.map(|c| c.0).unwrap_or(""), d.message);
    for note in &d.notes {
        out.push_str(&format!("\n{note}"));
    }
    out
}

#[test]
fn registry_dependencies() {
    let cwd = scratch("registry");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("router", "1.1.0", &[])
        .add("router", "1.2.0", &[])
        .add("router", "1.3.0", &[("json_extra", "^0.3")])
        .add("router", "2.0.0", &[])
        .add("json_extra", "0.3.1", &[])
        .add("json_extra", "0.4.0", &[]);
    write_package(&cwd.join("app"), "app", "router = \"1.2\"\n");
    let (graph, report) = resolve_with(&cwd, &reg.url(), false).unwrap();
    assert_eq!(versions(&graph), ["json_extra 0.3.1", "router 1.3.0"]);
    assert_eq!(report.fetched, ["json_extra 0.3.1", "router 1.3.0"]);
    let router = graph.packages[0].dependency("router").unwrap();
    let json = graph.packages[router].dependency("json_extra").unwrap();
    assert_eq!(graph.packages[json].name(), "json_extra");
    assert!(graph.packages[router].lib_file().is_file());
    let lock = fs::read_to_string(cwd.join("app/.brimlock")).unwrap();
    // As a TOML string (Windows paths have `\`).
    let source = format!("registry+{}", reg.url()).replace('\\', "\\\\");
    assert!(lock.contains(&format!(
        "name = \"router\"\nversion = \"1.3.0\"\nsource = \"{source}\"\nchecksum = \""
    )));

    // The lock file keeps 1.3.0, without the registry.
    reg.add("router", "1.4.0", &[("json_extra", "^0.3")]);
    fs::rename(cwd.join("registry"), cwd.join("registry-away")).unwrap();
    let (graph, report) = resolve_with(&cwd, &reg.url(), false).unwrap();
    assert_eq!(versions(&graph), ["json_extra 0.3.1", "router 1.3.0"]);
    assert!(report.fetched.is_empty() && !report.lock_written);
    fs::rename(cwd.join("registry-away"), cwd.join("registry")).unwrap();

    // `brim update` picks the newest.
    let (graph, _) = resolve_with(&cwd, &reg.url(), true).unwrap();
    assert_eq!(versions(&graph), ["json_extra 0.3.1", "router 1.4.0"]);

    // A changed requirement picks again.
    write_package(&cwd.join("app"), "app", "router = \"~1.2\"\n");
    let (graph, _) = resolve_with(&cwd, &reg.url(), false).unwrap();
    assert_eq!(versions(&graph), ["router 1.2.0"]);
}

#[test]
fn registry_requirements_meet() {
    let cwd = scratch("registry-meet");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("a", "1.0.0", &[("c", "^1.0")])
        .add("b", "1.0.0", &[("c", "~1.1")])
        .add("c", "1.0.0", &[])
        .add("c", "1.1.0", &[])
        .add("c", "1.1.5", &[])
        .add("c", "1.2.0", &[]);
    reg.yank("c", "1.1.5");
    write_package(&cwd.join("app"), "app", "a = \"1\"\nb = \"1\"\n");
    let (graph, _) = resolve_with(&cwd, &reg.url(), false).unwrap();
    assert_eq!(versions(&graph), ["a 1.0.0", "b 1.0.0", "c 1.1.0"]);
}

#[test]
fn registry_problems() {
    let cwd = scratch("registry-problems");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("router", "1.3.0", &[("json_extra", "^0.3")])
        .add("json_extra", "0.3.1", &[])
        .add("json_extra", "0.4.0", &[]);

    write_package(
        &cwd.join("app"),
        "app",
        "router = \"1.3\"\njson_extra = \"0.4\"\n",
    );
    let failure = resolve_with(&cwd, &reg.url(), false).unwrap_err();
    let text = failure_text(&failure);
    assert!(
        text.starts_with("M0014: no version of `json_extra`"),
        "{text}"
    );
    assert!(text.contains("app requires `json_extra 0.4`"), "{text}");
    assert!(
        text.contains("router 1.3.0 requires `json_extra ^0.3`"),
        "{text}"
    );
    assert!(text.contains("available versions: 0.3.1, 0.4.0"), "{text}");
    assert!(failure.diagnostic.span().is_some());

    write_package(&cwd.join("app"), "app", "nothing = \"1\"\n");
    let text = failure_text(&resolve_with(&cwd, &reg.url(), false).unwrap_err());
    assert!(
        text.starts_with("M0013: the registry has no package `nothing`"),
        "{text}"
    );

    write_package(&cwd.join("app"), "app", "router = \"2\"\n");
    let text = failure_text(&resolve_with(&cwd, &reg.url(), false).unwrap_err());
    assert!(text.contains("available versions: 1.3.0"), "{text}");

    reg.corrupt("json_extra", "0.3.1");
    write_package(&cwd.join("app"), "app", "router = \"1\"\n");
    let text = failure_text(&resolve_with(&cwd, &reg.url(), false).unwrap_err());
    assert!(
        text.starts_with("M0008: cannot get dependency `json_extra` 0.3.1"),
        "{text}"
    );
    assert!(text.contains("checksum"), "{text}");

    // A local package and a registry package with the same name.
    write_package(
        &cwd.join("app"),
        "app",
        "router = { path = \"../router\" }\nother = { path = \"../other\" }\n",
    );
    write_package(&cwd.join("router"), "router", "");
    write_package(&cwd.join("other"), "other", "router = \"1\"\n");
    let text = failure_text(&resolve_with(&cwd, &reg.url(), false).unwrap_err());
    assert!(
        text.starts_with("M0009: two different packages are named `router`"),
        "{text}"
    );
}

#[test]
fn archives_stay_inside() {
    let cwd = scratch("archives");
    let good = registry::pack(
        Path::new("."),
        &[],
        &[(".brimproj".into(), b"x".to_vec())],
        "a-1.0.0",
    )
    .unwrap();
    registry::unpack(&good, "a-1.0.0", &cwd.join("good")).unwrap();
    assert_eq!(fs::read(cwd.join("good/.brimproj")).unwrap(), b"x");
    let err = registry::unpack(&good, "b-1.0.0", &cwd.join("bad")).unwrap_err();
    assert!(err.contains("outside `b-1.0.0/`"), "{err}");
    // The same archive twice is the same bytes.
    let again = registry::pack(
        Path::new("."),
        &[],
        &[(".brimproj".into(), b"x".to_vec())],
        "a-1.0.0",
    )
    .unwrap();
    assert_eq!(good, again);
}

fn read_member(cwd: &Path, dir: &Path, span: Option<brim_diagnostics::Span>) -> Member {
    let path = dir.join(".brimproj");
    let text = fs::read_to_string(cwd.join(&path)).unwrap();
    let file = SourceFile::new(slash(&path), text);
    Member {
        dir: dir.to_path_buf(),
        manifest: brim_manifest::parse(file.text()).unwrap(),
        manifest_file: file,
        span,
    }
}

#[test]
fn workspaces() {
    let cwd = scratch("workspace");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("router", "1.0.0", &[]).add("router", "1.1.0", &[]);
    fs::create_dir_all(cwd.join("ws")).unwrap();
    fs::write(
        cwd.join("ws/.brimproj"),
        "[workspace]\nmembers = [\"apps/*\", \"lib\"]\n",
    )
    .unwrap();
    write_package(
        &cwd.join("ws/apps/api"),
        "api",
        "lib = { path = \"../../lib\", version = \"0.1\" }\nrouter = \"1\"\n",
    );
    write_package(
        &cwd.join("ws/apps/cli"),
        "cli",
        "lib = { path = \"../../lib\" }\n",
    );
    write_package(&cwd.join("ws/lib"), "lib", "router = \"~1.0\"\n");

    let root_text = fs::read_to_string(cwd.join("ws/.brimproj")).unwrap();
    let root_file = SourceFile::new("ws/.brimproj", root_text);
    let root = brim_manifest::parse_root(root_file.text()).unwrap();
    let dirs = member_dirs(
        &cwd,
        Path::new("ws"),
        root.workspace.as_ref().unwrap(),
        &root_file,
    )
    .unwrap();
    let shown: Vec<String> = dirs.iter().map(|(d, _)| slash(d)).collect();
    assert_eq!(shown, ["ws/apps/api", "ws/apps/cli", "ws/lib"]);
    let members: Vec<Member> = dirs
        .iter()
        .map(|(d, span)| read_member(&cwd, d, Some(*span)))
        .collect();
    let options = Options {
        cwd: cwd.clone(),
        home: cwd.join("home"),
        update: false,
        registry: Some(reg.url()),
        offline: false,
    };
    let (graphs, _) = resolve_workspace(Path::new("ws"), root_file, &members, &options).unwrap();
    let names =
        |g: &Graph| -> Vec<String> { g.packages.iter().map(|p| p.name().to_string()).collect() };
    assert_eq!(names(&graphs[0]), ["api", "lib", "router"]);
    assert_eq!(names(&graphs[1]), ["cli", "lib", "router"]);
    assert_eq!(names(&graphs[2]), ["lib", "router"]);
    // One version for the whole workspace, in one lock file at the root.
    for g in &graphs {
        let router = g.packages.iter().find(|p| p.name() == "router").unwrap();
        assert_eq!(router.manifest.package.version, "1.0.0");
    }
    let api = &graphs[0];
    assert_eq!(
        api.packages[api.packages[0].dependency("lib").unwrap()].name(),
        "lib"
    );
    let lock = fs::read_to_string(cwd.join("ws/.brimlock")).unwrap();
    assert!(lock.contains("source = \"path+apps/api\""), "{lock}");
    assert!(
        lock.contains("name = \"router\"\nversion = \"1.0.0\""),
        "{lock}"
    );
    assert!(!cwd.join("ws/apps/api/.brimlock").exists());

    // A member finds its workspace; a package outside does not.
    let (found, file) = find_root(&cwd, Path::new("ws/apps/cli")).unwrap();
    assert_eq!(found, PathBuf::from("ws"));
    assert_eq!(file.name(), "ws/.brimproj");
    write_package(&cwd.join("ws/other"), "other", "");
    assert!(find_root(&cwd, Path::new("ws/other")).is_none());

    // Two members with one name.
    write_package(&cwd.join("ws/apps/cli"), "api", "");
    let members: Vec<Member> = dirs
        .iter()
        .map(|(d, span)| read_member(&cwd, d, Some(*span)))
        .collect();
    let root_file = SourceFile::new(
        "ws/.brimproj",
        fs::read_to_string(cwd.join("ws/.brimproj")).unwrap(),
    );
    let failure = resolve_workspace(Path::new("ws"), root_file, &members, &options).unwrap_err();
    assert_eq!(
        failure.diagnostic.message,
        "two workspace members are named `api`"
    );
}

#[test]
fn a_changed_version_is_not_believed() {
    let cwd = scratch("changed");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("router", "1.0.0", &[])
        .add("json_extra", "0.3.1", &[]);
    write_package(&cwd.join("app"), "app", "router = \"1.0\"\n");
    resolve_with(&cwd, &reg.url(), false).unwrap();
    let lock = fs::read_to_string(cwd.join("app/.brimlock")).unwrap();

    // The registry swaps 1.0.0 for other contents, and the cache is empty
    // (a new machine, CI).
    reg.index.remove("router");
    reg.add("router", "1.0.0", &[("json_extra", "^0.3")]);
    fs::remove_dir_all(cwd.join("home")).unwrap();
    let failure = resolve_with(&cwd, &reg.url(), false).unwrap_err();
    assert_eq!(failure.diagnostic.code.map(|c| c.0), Some("M0015"));
    assert_eq!(fs::read_to_string(cwd.join("app/.brimlock")).unwrap(), lock);

    // `brim update` records the new archive.
    resolve_with(&cwd, &reg.url(), true).unwrap();
    assert_ne!(fs::read_to_string(cwd.join("app/.brimlock")).unwrap(), lock);
}

#[test]
fn offline_resolution_fetches_nothing() {
    let cwd = scratch("offline");
    let mut reg = TestRegistry::new(cwd.join("registry"));
    reg.add("router", "1.0.0", &[]);
    write_package(&cwd.join("app"), "app", "router = \"1.0\"\n");
    let path = Path::new("app").join(".brimproj");
    let file = SourceFile::new(
        path.display().to_string(),
        fs::read_to_string(cwd.join(&path)).unwrap(),
    );
    let manifest = brim_manifest::parse(file.text()).unwrap();
    let options = Options {
        cwd: cwd.clone(),
        home: cwd.join("home"),
        update: false,
        registry: Some(reg.url()),
        offline: true,
    };
    let failure = resolve(Path::new("app"), manifest, file, &options).unwrap_err();
    assert!(
        failure
            .diagnostic
            .notes
            .iter()
            .any(|n| n.contains("not fetched yet")),
        "{failure:?}"
    );
    assert!(!cwd.join("home").exists());
    assert!(!cwd.join("app/.brimlock").exists());
}
