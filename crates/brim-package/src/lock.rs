//! The lock file, `brim.lock`.
//!
//! ```toml
//! # This file is written by brim. Do not edit it.
//! version = 1
//!
//! [[package]]
//! name = "http"
//! version = "1.0.0"
//! source = "git+https://example.com/http.git?tag=v1.0.0"
//! commit = "4f0c..."
//!
//! [[package]]
//! name = "router"
//! version = "1.2.0"
//! source = "registry+https://registry.ordnary.com"
//! checksum = "9a1e..."
//! ```

use std::path::Path;

use brim_diagnostics::{Diagnostic, SourceFile, Span};
use brim_manifest::{GitRef, Version};
use serde::Deserialize;

use crate::{Failure, Graph};

pub const LOCK_FILE: &str = "brim.lock";

const FORMAT_VERSION: u32 = 1;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockFile {
    #[serde(default)]
    version: u32,
    #[serde(default, rename = "package")]
    packages: Vec<LockedPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPackage {
    name: String,
    version: String,
    source: String,
    commit: Option<String>,
    checksum: Option<String>,
}

impl LockFile {
    /// The locked commit of a git dependency.
    pub(crate) fn commit(&self, name: &str, source: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|p| p.name == name && p.source == source)
            .and_then(|p| p.commit.as_deref())
    }

    /// The locked version and checksum of a registry package.
    pub(crate) fn registry(&self, name: &str, source: &str) -> Option<(Version, &str)> {
        let p = self
            .packages
            .iter()
            .find(|p| p.name == name && p.source == source)?;
        Some((Version::parse(&p.version)?, p.checksum.as_deref()?))
    }
}

/// How a registry appears in the lock file.
pub(crate) fn registry_source(url: &str) -> String {
    format!("registry+{url}")
}

/// How a git dependency appears in the lock file.
pub(crate) fn git_source(url: &str, reference: &GitRef) -> String {
    match reference {
        GitRef::DefaultBranch => format!("git+{url}"),
        GitRef::Branch(b) => format!("git+{url}?branch={b}"),
        GitRef::Tag(t) => format!("git+{url}?tag={t}"),
        GitRef::Rev(r) => format!("git+{url}?rev={r}"),
    }
}

/// Reads the lock file, if there is one.
pub(crate) fn read(full: &Path, shown: &Path) -> Result<LockFile, Failure> {
    let Ok(text) = std::fs::read_to_string(full) else {
        return Ok(LockFile::default());
    };
    let shown = shown.display().to_string().replace('\\', "/");
    let file = SourceFile::new(shown.clone(), text);
    let lock: LockFile = toml::from_str(file.text()).map_err(|e| {
        let mut diag = Diagnostic::error(format!("invalid lock file: {}", e.message().trim()))
            .with_code("M0010")
            .with_help(format!("delete {shown} to create it again"));
        if let Some(range) = e.span() {
            diag = diag.with_primary(Span::new(range.start, range.end), "");
        }
        Failure::in_file(diag, &file)
    })?;
    if lock.version != FORMAT_VERSION {
        return Err(Failure::in_file(
            Diagnostic::error(format!("unsupported lock file version {}", lock.version))
                .with_code("M0010")
                .with_help(format!("delete {shown} to create it again")),
            &file,
        ));
    }
    Ok(lock)
}

/// The lock file for a resolved project.
pub(crate) fn render(graph: &Graph) -> String {
    let mut entries: Vec<String> = graph.packages[1..]
        .iter()
        .map(|p| {
            let source = match (&p.git, find_source(graph, p.name())) {
                _ if p.registry.is_some() => {
                    let (url, checksum) = p.registry.as_ref().expect("checked");
                    format!(
                        "source = {}\nchecksum = {}\n",
                        quote(&registry_source(url)),
                        quote(checksum)
                    )
                }
                (Some((url, commit)), Some(brim_manifest::Source::Git { reference, .. })) => {
                    format!(
                        "source = {}\ncommit = {}\n",
                        quote(&git_source(url, reference)),
                        quote(commit)
                    )
                }
                // As written in the manifest, so the lock file does not
                // depend on the working directory.
                (_, Some(brim_manifest::Source::Path(path))) => format!(
                    "source = {}\n",
                    quote(&format!(
                        "path+{}",
                        path.display().to_string().replace('\\', "/")
                    ))
                ),
                _ => String::new(),
            };
            format!(
                "[[package]]\nname = {}\nversion = {}\n{source}",
                quote(p.name()),
                quote(&p.manifest.package.version)
            )
        })
        .collect();
    entries.sort();
    let mut out =
        format!("# This file is written by brim. Do not edit it.\nversion = {FORMAT_VERSION}\n");
    for entry in entries {
        out.push('\n');
        out.push_str(&entry);
    }
    out
}

/// The source a package was first required with.
fn find_source<'a>(graph: &'a Graph, name: &str) -> Option<&'a brim_manifest::Source> {
    graph
        .packages
        .iter()
        .flat_map(|p| &p.manifest.dependencies)
        .find(|d| d.name == name)
        .map(|d| &d.source)
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
