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
//! ```
//!
//! This is Brim's own, proposed format (see `docs/language-spec.md`). Unknown
//! keys are rejected so typos are caught early. A `[dependencies]` table is
//! recognised but rejected for now: dependency resolution and a package
//! registry are planned for a later milestone, and accepting the table
//! silently would suggest they work.

use std::path::{Component, Path, PathBuf};

use brim_diagnostics::{Diagnostic, Span};
use serde::Deserialize;
use toml::Spanned;

/// The file name of a Brim project manifest.
pub const MANIFEST_FILE: &str = "brim.toml";

/// The entry point used when `[build] entry` is not given.
pub const DEFAULT_ENTRY: &str = "src/main.brim";

/// Language editions this compiler understands.
pub const EDITIONS: &[&str] = &["2026"];

/// A validated manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub package: Package,
    pub build: Build,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    package: RawPackage,
    build: Option<RawBuild>,
    dependencies: Option<Spanned<toml::Table>>,
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
                .with_note(
                    "package names start with a lowercase ASCII letter and contain only \
                     lowercase letters, digits, `_` and `-` (at most 64 characters)",
                ),
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

    let entry = match raw.build.and_then(|b| b.entry) {
        None => PathBuf::from(DEFAULT_ENTRY),
        Some(entry) => {
            if let Err(problem) = check_entry(entry.get_ref()) {
                diags.push(
                    Diagnostic::error(format!("invalid entry path `{}`", entry.get_ref()))
                        .with_code("M0005")
                        .with_primary(span(&entry), problem),
                );
            }
            PathBuf::from(entry.into_inner())
        }
    };

    if let Some(deps) = raw.dependencies {
        diags.push(
            Diagnostic::error("dependencies are not supported yet")
                .with_code("M0006")
                .with_primary(Span::new(deps.span().start, deps.span().end), "")
                .with_note("package dependencies and a registry are planned; see docs/roadmap.md"),
        );
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
        build: Build { entry },
    })
}

fn valid_name(name: &str) -> bool {
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
        return Err("the entry path must be relative to brim.toml");
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err("the entry path must stay inside the package directory");
    }
    if path.extension().is_none_or(|ext| ext != "brim") {
        return Err("the entry file must have the `.brim` extension");
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
    fn dependencies_are_rejected_explicitly() {
        assert_eq!(
            codes(&format!("{VALID}\n[dependencies]\nhttp = \"1.0\"\n")),
            vec!["M0006"]
        );
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
