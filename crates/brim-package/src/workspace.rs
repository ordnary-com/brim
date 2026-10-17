//! Workspaces: packages developed together, with one lock file.
//!
//! The workspace root's `brim.toml` has `[workspace] members = [...]`, and
//! may be a package itself. [`resolve_workspace`] resolves all members at
//! once, as if one package depended on all of them, so they share
//! `brim.lock` in the root and every dependency has one version in the
//! whole workspace. Each member then gets its own [`Graph`].

use std::path::{Path, PathBuf};

use brim_diagnostics::{Diagnostic, SourceFile, Span};
use brim_manifest::{Build, Dependency, Manifest, Source};

use crate::{Failure, Graph, Options, Package, Report, slash};

/// A member of a workspace, read from its manifest.
#[derive(Clone, Debug)]
pub struct Member {
    /// The member's directory, relative to the working directory (like
    /// [`Package::dir`]).
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub manifest_file: SourceFile,
    /// The entry of `[workspace] members` that names it, in the root's
    /// manifest; `None` for the root package.
    pub span: Option<Span>,
}

/// The member directories of the workspace in `root` (relative to the
/// working directory), in the order of `members`, with `*` expanded.
pub fn member_dirs(
    cwd: &Path,
    root: &Path,
    workspace: &brim_manifest::Workspace,
    root_file: &SourceFile,
) -> Result<Vec<(PathBuf, Span)>, Failure> {
    let mut out = Vec::new();
    for (member, span) in &workspace.members {
        if member.file_name().is_some_and(|n| n == "*") {
            let parent = root.join(member.parent().unwrap_or(Path::new("")));
            let mut found: Vec<PathBuf> = std::fs::read_dir(cwd.join(&parent))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.join(brim_manifest::MANIFEST_FILE).is_file())
                        .filter_map(|p| p.file_name().map(|n| parent.join(n)))
                        .collect()
                })
                .unwrap_or_default();
            found.sort();
            out.extend(found.into_iter().map(|dir| (shown(&dir), *span)));
        } else {
            let dir = shown(&root.join(member));
            if !cwd.join(&dir).join(brim_manifest::MANIFEST_FILE).is_file() {
                return Err(Failure::in_file(
                    Diagnostic::error(format!(
                        "the workspace member `{}` has no brim.toml",
                        slash(member)
                    ))
                    .with_code("M0012")
                    .with_primary(*span, ""),
                    root_file,
                ));
            }
            out.push((dir, *span));
        }
    }
    Ok(out)
}

/// A directory as the compiler shows paths: normalized, with `/` on every
/// platform.
fn shown(dir: &Path) -> PathBuf {
    PathBuf::from(slash(&crate::normalize(dir)))
}

/// Finds the workspace a package in `dir` (relative to `cwd`) belongs to:
/// the nearest ancestor whose `brim.toml` has a `[workspace]` listing it.
/// Returns the root's directory and manifest file.
pub fn find_root(cwd: &Path, dir: &Path) -> Option<(PathBuf, SourceFile)> {
    let full = cwd.join(dir);
    let canonical = full.canonicalize().ok()?;
    for ancestor in canonical.ancestors().skip(1) {
        let manifest = ancestor.join(brim_manifest::MANIFEST_FILE);
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(root) = brim_manifest::parse_root(&text) else {
            continue;
        };
        let Some(workspace) = &root.workspace else {
            continue;
        };
        // The root relative to the working directory, when it is below it.
        let cwd_full = cwd.canonicalize().ok()?;
        let shown_dir = ancestor
            .strip_prefix(&cwd_full)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| ancestor.to_path_buf());
        let file = SourceFile::new(slash(&shown_dir.join(brim_manifest::MANIFEST_FILE)), text);
        let members = member_dirs(cwd, &shown_dir, workspace, &file).ok()?;
        let listed = members
            .iter()
            .any(|(m, _)| cwd.join(m).canonicalize().ok().as_deref() == Some(canonical.as_path()));
        return listed.then_some((shown_dir, file));
    }
    None
}

/// Resolves every member of the workspace in `root` together. Returns a
/// graph per member, in the order of `members`.
pub fn resolve_workspace(
    root: &Path,
    root_file: SourceFile,
    members: &[Member],
    options: &Options,
) -> Result<(Vec<Graph>, Report), Failure> {
    // One package that depends on every member, by path.
    let mut names: Vec<&str> = Vec::new();
    for member in members {
        let name = member.manifest.package.name.as_str();
        if names.contains(&name) {
            let diag = Diagnostic::error(format!("two workspace members are named `{name}`"))
                .with_code("M0009");
            return Err(match member.span {
                Some(span) => Failure::in_file(diag.with_primary(span, ""), &root_file),
                None => Failure::new(diag),
            });
        }
        names.push(name);
    }
    let dependencies = members
        .iter()
        .map(|m| {
            let relative = relative_to(&m.dir, root);
            Dependency {
                name: m.manifest.package.name.clone(),
                source: Source::Path(relative),
                version: None,
                span: m.span.unwrap_or(Span::new(0, 0)),
            }
        })
        .collect();
    let manifest = Manifest {
        package: brim_manifest::Package {
            // No package can have an empty name, so it is never taken.
            name: String::new(),
            version: "0.0.0".into(),
            edition: brim_manifest::EDITIONS[0].into(),
            description: None,
            license: None,
            repository: None,
        },
        build: Build {
            entry: brim_manifest::DEFAULT_ENTRY.into(),
            lib: brim_manifest::DEFAULT_LIB.into(),
            c_sources: Vec::new(),
            link: Vec::new(),
        },
        dependencies,
    };
    let (all, report) = crate::resolve(root, manifest, root_file, options)?;
    let graphs = members
        .iter()
        .map(|m| {
            let index = all.packages[0]
                .dependency(&m.manifest.package.name)
                .expect("every member is a dependency");
            let mut graph = subgraph(&all, index);
            // The member's own manifest file, as read (with its name as
            // the user sees it).
            graph.packages[0].manifest_file = m.manifest_file.clone();
            graph
        })
        .collect();
    Ok((graphs, report))
}

/// The packages reachable from `root`, with `root` first.
fn subgraph(graph: &Graph, root: usize) -> Graph {
    let mut order = vec![root];
    let mut next = 0;
    while next < order.len() {
        for &(_, dep) in &graph.packages[order[next]].dependencies {
            if !order.contains(&dep) {
                order.push(dep);
            }
        }
        next += 1;
    }
    let packages = order
        .iter()
        .map(|&old| {
            let mut package: Package = graph.packages[old].clone();
            for (_, dep) in &mut package.dependencies {
                *dep = order.iter().position(|&o| o == *dep).expect("reachable");
            }
            package
        })
        .collect();
    Graph { packages }
}

/// `path` relative to `base`, both relative to the same directory.
fn relative_to(path: &Path, base: &Path) -> PathBuf {
    if base.as_os_str().is_empty() || base == Path::new(".") {
        return path.to_path_buf();
    }
    let path: Vec<_> = path.components().collect();
    let base: Vec<_> = base.components().collect();
    let common = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..base.len() {
        out.push("..");
    }
    for c in &path[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}
