//! The `brim.toml` project manifest.
//!
//! ```toml
//! [package]
//! name = "hello"
//! version = "0.1.0"
//! edition = "2026"
//!
//! [build]
//! entry = "src/main.brim"   # optional; this is the default
//! lib = "src/lib.brim"      # optional; this is the default
//!
//! [dependencies]
//! geometry = { path = "../geometry" }
//! http = { git = "https://example.com/http.git", tag = "v1.0.0" }
//! ```
//!
//! This is Brim's own, proposed format (see `docs/language-spec.md`). Unknown
//! keys are rejected so typos are caught early. Dependencies come from a
//! directory or a git repository; there is no package registry yet.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use brim_diagnostics::{Diagnostic, Span};
use serde::Deserialize;
use toml::Spanned;

/// The file name of a Brim project manifest.
pub const MANIFEST_FILE: &str = "brim.toml";

/// The entry point used when `[build] entry` is not given.
pub const DEFAULT_ENTRY: &str = "src/main.brim";

/// The library root used when `[build] lib` is not given: the module other
/// packages get with `import <package>`.
pub const DEFAULT_LIB: &str = "src/lib.brim";

/// Language editions this compiler understands.
pub const EDITIONS: &[&str] = &["2026"];

/// A validated manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub package: Package,
    pub build: Build,
    /// In name order.
    pub dependencies: Vec<Dependency>,
}

/// An entry of `[dependencies]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// The name the package is imported by; it must be the package's own
    /// name.
    pub name: String,
    pub source: Source,
    /// The entry in the manifest.
    pub span: Span,
}

/// Where a dependency comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A directory with a `brim.toml`, relative to the depending manifest
    /// (or absolute).
    Path(PathBuf),
    Git {
        url: String,
        reference: GitRef,
    },
}

/// The version of a git dependency to use.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GitRef {
    /// The repository's default branch (`HEAD`).
    DefaultBranch,
    Branch(String),
    Tag(String),
    /// A commit.
    Rev(String),
}

impl GitRef {
    /// The revision to ask git for.
    pub fn revision(&self) -> &str {
        match self {
            GitRef::DefaultBranch => "HEAD",
            GitRef::Branch(name) | GitRef::Tag(name) | GitRef::Rev(name) => name,
        }
    }
}

impl std::fmt::Display for GitRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitRef::DefaultBranch => f.write_str("default branch"),
            GitRef::Branch(name) => write!(f, "branch {name}"),
            GitRef::Tag(name) => write!(f, "tag {name}"),
            GitRef::Rev(name) => write!(f, "rev {name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    /// Path of the entry source file, relative to the manifest's directory.
    pub entry: PathBuf,
    /// Path of the library root, relative to the manifest's directory.
    pub lib: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    package: RawPackage,
    build: Option<RawBuild>,
    dependencies: Option<BTreeMap<Spanned<String>, Spanned<toml::Value>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
    name: Spanned<String>,
    version: Spanned<String>,
    edition: Spanned<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBuild {
    entry: Option<Spanned<String>>,
    lib: Option<Spanned<String>>,
}

/// Parses and validates manifest text. Diagnostics point into `src`.
pub fn parse(src: &str) -> Result<Manifest, Vec<Diagnostic>> {
    let raw: RawManifest = toml::from_str(src).map_err(|e| {
        let mut diag = Diagnostic::error(format!("invalid manifest: {}", e.message().trim()))
            .with_code("M0001");
        if let Some(range) = e.span() {
            diag = diag.with_primary(Span::new(range.start, range.end), "");
        }
        vec![diag]
    })?;

    let mut diags = Vec::new();
    let span = |s: &Spanned<String>| Span::new(s.span().start, s.span().end);

    let name = raw.package.name.get_ref();
    if !valid_name(name) {
        diags.push(
            Diagnostic::error(format!("invalid package name `{name}`"))
                .with_code("M0002")
                .with_primary(span(&raw.package.name), "")
                .with_note(NAME_RULES),
        );
    }

    let version = raw.package.version.get_ref();
    if !valid_version(version) {
        diags.push(
            Diagnostic::error(format!("invalid package version `{version}`"))
                .with_code("M0003")
                .with_primary(span(&raw.package.version), "")
                .with_note(
                    "versions have the form MAJOR.MINOR.PATCH, e.g. `0.1.0` or `1.2.3-beta.1`",
                ),
        );
    }

    let edition = raw.package.edition.get_ref();
    if !EDITIONS.contains(&edition.as_str()) {
        diags.push(
            Diagnostic::error(format!("unsupported edition `{edition}`"))
                .with_code("M0004")
                .with_primary(span(&raw.package.edition), "")
                .with_note(format!(
                    "this compiler supports: {}",
                    EDITIONS
                        .iter()
                        .map(|e| format!("`{e}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
        );
    }

    let (entry, lib) = match raw.build {
        None => (None, None),
        Some(build) => (build.entry, build.lib),
    };
    let mut source_path = |path: Option<Spanned<String>>, default: &str| match path {
        None => PathBuf::from(default),
        Some(path) => {
            if let Err(problem) = check_entry(path.get_ref()) {
                diags.push(
                    Diagnostic::error(format!("invalid source path `{}`", path.get_ref()))
                        .with_code("M0005")
                        .with_primary(span(&path), problem),
                );
            }
            PathBuf::from(path.into_inner())
        }
    };
    let entry = source_path(entry, DEFAULT_ENTRY);
    let lib = source_path(lib, DEFAULT_LIB);

    let mut dependencies = Vec::new();
    for (name, value) in raw.dependencies.unwrap_or_default() {
        let entry_span = Span::new(name.span().start, value.span().end);
        if !valid_import_name(name.get_ref()) {
            diags.push(
                Diagnostic::error(format!("invalid dependency name `{}`", name.get_ref()))
                    .with_code("M0007")
                    .with_primary(span(&name), "")
                    .with_note(
                        "dependencies are imported by name, so their names start with a \
                         lowercase ASCII letter and contain only lowercase letters, digits \
                         and `_`",
                    ),
            );
            continue;
        }
        match dependency_source(value.get_ref()) {
            Ok(source) => dependencies.push(Dependency {
                name: name.into_inner(),
                source,
                span: entry_span,
            }),
            Err((message, help)) => diags.push(
                Diagnostic::error(message)
                    .with_code("M0006")
                    .with_primary(Span::new(value.span().start, value.span().end), "")
                    .with_help(help),
            ),
        }
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(Manifest {
        package: Package {
            name: raw.package.name.into_inner(),
            version: raw.package.version.into_inner(),
            edition: raw.package.edition.into_inner(),
        },
        build: Build { entry, lib },
        dependencies,
    })
}

/// The source of a dependency entry, or an error message and help.
fn dependency_source(value: &toml::Value) -> Result<Source, (String, &'static str)> {
    const FORMS: &str = "write `{ path = \"../dir\" }` or `{ git = \"URL\" }`, optionally with `tag`, `branch` or `rev`";
    let table = match value {
        toml::Value::String(_) => {
            return Err((
                "version requirements need a package registry, which does not exist yet".into(),
                FORMS,
            ));
        }
        toml::Value::Table(table) => table,
        _ => return Err(("a dependency must be a table".into(), FORMS)),
    };
    let mut strings = BTreeMap::new();
    for (key, value) in table {
        if !matches!(key.as_str(), "path" | "git" | "tag" | "branch" | "rev") {
            return Err((format!("unknown dependency key `{key}`"), FORMS));
        }
        let Some(text) = value.as_str() else {
            return Err((format!("`{key}` must be a string"), FORMS));
        };
        strings.insert(key.as_str(), text.to_string());
    }
    let refs: Vec<&str> = ["tag", "branch", "rev"]
        .into_iter()
        .filter(|k| strings.contains_key(k))
        .collect();
    match (strings.remove("path"), strings.remove("git")) {
        (Some(_), Some(_)) => Err(("a dependency has either `path` or `git`".into(), FORMS)),
        (None, None) => Err(("a dependency needs `path` or `git`".into(), FORMS)),
        (Some(path), None) => {
            if let Some(key) = refs.first() {
                return Err((format!("`{key}` only applies to git dependencies"), FORMS));
            }
            Ok(Source::Path(PathBuf::from(path)))
        }
        (None, Some(url)) => {
            if refs.len() > 1 {
                return Err((
                    "use only one of `tag`, `branch` and `rev`".into(),
                    "they all select the version to use",
                ));
            }
            let reference = match refs.first() {
                None => GitRef::DefaultBranch,
                Some(&"tag") => GitRef::Tag(strings.remove("tag").unwrap_or_default()),
                Some(&"branch") => GitRef::Branch(strings.remove("branch").unwrap_or_default()),
                Some(_) => GitRef::Rev(strings.remove("rev").unwrap_or_default()),
            };
            Ok(Source::Git { url, reference })
        }
    }
}

/// Whether a package name can be written in `import`.
pub fn valid_import_name(name: &str) -> bool {
    valid_name(name) && !name.contains('-')
}

/// What [`valid_name`] requires, for messages.
pub const NAME_RULES: &str = "package names start with a lowercase ASCII letter and contain only \
                              lowercase letters, digits, `_` and `-` (at most 64 characters)";

/// Whether `name` can be a package name.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_version(version: &str) -> bool {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    let numeric_ok = numbers.len() == 3
        && numbers.iter().all(|n| {
            !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit())
                && (n.len() == 1 || !n.starts_with('0'))
                && n.parse::<u64>().is_ok()
        });
    let pre_ok = pre.is_none_or(|p| {
        p.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    });
    numeric_ok && pre_ok
}

fn check_entry(entry: &str) -> Result<(), &'static str> {
    let path = Path::new(entry);
    if path.is_absolute() || entry.starts_with('/') || entry.starts_with('\\') {
        return Err("the path must be relative to brim.toml");
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err("the path must stay inside the package directory");
    }
    if path.extension().is_none_or(|ext| ext != "brim") {
        return Err("source files have the `.brim` extension");
    }
    Ok(())
}

/// Searches `start` and its ancestors for a `brim.toml`.
pub fn find(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(MANIFEST_FILE))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
[package]
name = "hello"
version = "0.1.0"
edition = "2026"

[build]
entry = "src/app.brim"
"#;

    fn codes(src: &str) -> Vec<&'static str> {
        parse(src)
            .unwrap_err()
            .into_iter()
            .map(|d| d.code.unwrap().0)
            .collect()
    }

    #[test]
    fn parses_valid_manifest() {
        let m = parse(VALID).unwrap();
        assert_eq!(m.package.name, "hello");
        assert_eq!(m.package.version, "0.1.0");
        assert_eq!(m.package.edition, "2026");
        assert_eq!(m.build.entry, PathBuf::from("src/app.brim"));
    }

    #[test]
    fn entry_defaults() {
        let m =
            parse("[package]\nname = \"a\"\nversion = \"1.0.0\"\nedition = \"2026\"\n").unwrap();
        assert_eq!(m.build.entry, PathBuf::from(DEFAULT_ENTRY));
        let m =
            parse("[package]\nname = \"a\"\nversion = \"1.0.0\"\nedition = \"2026\"\n[build]\n")
                .unwrap();
        assert_eq!(m.build.entry, PathBuf::from(DEFAULT_ENTRY));
    }

    #[test]
    fn missing_and_unknown_fields() {
        let errs = parse("[package]\nname = \"a\"\nedition = \"2026\"\n").unwrap_err();
        assert_eq!(errs[0].code.unwrap().0, "M0001");
        assert!(
            errs[0].message.contains("missing field `version`"),
            "{}",
            errs[0].message
        );

        let errs = parse(&VALID.replace("edition", "editon")).unwrap_err();
        assert!(
            errs[0].message.contains("unknown field `editon`"),
            "{}",
            errs[0].message
        );
        assert!(errs[0].span().is_some());

        assert_eq!(codes("not toml at all ["), vec!["M0001"]);
        assert_eq!(codes(""), vec!["M0001"]);
    }

    #[test]
    fn validates_values() {
        assert_eq!(
            codes(&VALID.replace("\"hello\"", "\"Hello World\"")),
            vec!["M0002"]
        );
        assert_eq!(codes(&VALID.replace("\"0.1.0\"", "\"1.0\"")), vec!["M0003"]);
        assert_eq!(codes(&VALID.replace("\"2026\"", "\"2015\"")), vec!["M0004"]);
        assert_eq!(
            codes(&VALID.replace("src/app.brim", "../x.brim")),
            vec!["M0005"]
        );
        assert_eq!(
            codes(&VALID.replace("src/app.brim", "/abs.brim")),
            vec!["M0005"]
        );
        assert_eq!(
            codes(&VALID.replace("src/app.brim", "src/app.rs")),
            vec!["M0005"]
        );
        // Several problems are reported together.
        assert_eq!(
            codes(
                &VALID
                    .replace("\"hello\"", "\"9\"")
                    .replace("\"2026\"", "\"1\"")
            ),
            vec!["M0002", "M0004"]
        );
    }

    #[test]
    fn dependencies() {
        let m = parse(&format!(
            "{VALID}\n[dependencies]\nshapes = {{ path = \"../shapes\" }}\n\
             http = {{ git = \"https://example.com/http.git\", tag = \"v1\" }}\n\
             json = {{ git = \"file:///tmp/json\" }}\n"
        ))
        .unwrap();
        let deps: Vec<(&str, &Source)> = m
            .dependencies
            .iter()
            .map(|d| (d.name.as_str(), &d.source))
            .collect();
        assert_eq!(
            deps,
            vec![
                (
                    "http",
                    &Source::Git {
                        url: "https://example.com/http.git".into(),
                        reference: GitRef::Tag("v1".into())
                    }
                ),
                (
                    "json",
                    &Source::Git {
                        url: "file:///tmp/json".into(),
                        reference: GitRef::DefaultBranch
                    }
                ),
                ("shapes", &Source::Path("../shapes".into())),
            ]
        );
        assert_eq!(m.build.lib, PathBuf::from(DEFAULT_LIB));
    }

    #[test]
    fn invalid_dependencies() {
        let deps = |text: &str| codes(&format!("{VALID}\n[dependencies]\n{text}\n"));
        assert_eq!(deps("http = \"1.0\""), vec!["M0006"]);
        assert_eq!(deps("a = { path = \"x\", git = \"y\" }"), vec!["M0006"]);
        assert_eq!(deps("a = {}"), vec!["M0006"]);
        assert_eq!(deps("a = { path = \"x\", tag = \"v1\" }"), vec!["M0006"]);
        assert_eq!(
            deps("a = { git = \"x\", tag = \"v1\", rev = \"abc\" }"),
            vec!["M0006"]
        );
        assert_eq!(deps("a = { git = \"x\", version = \"1\" }"), vec!["M0006"]);
        assert_eq!(deps("a = { path = 1 }"), vec!["M0006"]);
        assert_eq!(deps("my-lib = { path = \"x\" }"), vec!["M0007"]);
    }

    #[test]
    fn version_rules() {
        for ok in ["0.0.0", "1.2.3", "10.20.30", "1.0.0-alpha", "1.0.0-beta.1"] {
            assert!(valid_version(ok), "{ok}");
        }
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "a.b.c",
            "1.0.0-",
            "1.0.0-a..b",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn name_rules() {
        for ok in ["a", "hello", "my-app_2"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "1app", "App", "my app", "é", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
