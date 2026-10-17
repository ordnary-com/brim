//! Packages and their dependencies.
//!
//! [`resolve`] starts at a project's manifest and finds every package it
//! depends on, directly or indirectly. Path dependencies are read where
//! they are. Git dependencies are cloned into a cache (`$BRIM_HOME/git`,
//! by default `~/.brim/git`) and checked out at one commit each. Registry
//! dependencies get a version that meets every requirement on them (see
//! [`registry`] and `docs/registry.md`) and are unpacked into
//! `$BRIM_HOME/registry`.
//!
//! The commits and versions are recorded in the project's lock file,
//! `brim.lock`, so later builds use exactly the same versions until
//! `brim update` asks for the newest ones. Path dependencies are listed in
//! the lock file too, but always used as they are.
//!
//! Every package in a project has a distinct name: a package's name is the
//! first part of its modules' names (`import geometry.shapes`).

mod git;
mod lock;
pub mod registry;
mod solve;
mod workspace;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use brim_diagnostics::{Diagnostic, SourceFile};
use brim_manifest::{Dependency, Manifest, Source, Version};

pub use lock::LOCK_FILE;
pub use workspace::{Member, find_root, member_dirs, resolve_workspace};

/// A package of a project.
#[derive(Clone, Debug)]
pub struct Package {
    /// The directory with the package's `brim.toml`: relative to the
    /// working directory for the project and its path dependencies, absolute
    /// for git checkouts.
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// The manifest's text, for messages.
    pub manifest_file: SourceFile,
    /// For each dependency, its index in [`Graph::packages`], in the order of
    /// the manifest.
    pub dependencies: Vec<(String, usize)>,
    /// For git packages: the URL and the commit.
    pub git: Option<(String, String)>,
    /// For registry packages: the registry's URL and the archive's
    /// checksum.
    pub registry: Option<(String, String)>,
}

impl Package {
    pub fn name(&self) -> &str {
        &self.manifest.package.name
    }

    /// The package's library root, `import <name>`.
    pub fn lib_file(&self) -> PathBuf {
        self.dir.join(&self.manifest.build.lib)
    }

    /// The directory of the library root, where `import <name>.a.b` looks
    /// for `a/b.brim`.
    pub fn lib_dir(&self) -> PathBuf {
        self.lib_file()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// The package `name` this package depends on.
    pub fn dependency(&self, name: &str) -> Option<usize> {
        self.dependencies
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, i)| i)
    }
}

/// A project and all its dependencies.
#[derive(Clone, Debug)]
pub struct Graph {
    /// The project's own package comes first.
    pub packages: Vec<Package>,
}

/// How to resolve.
#[derive(Clone, Debug)]
pub struct Options {
    /// The working directory, which relative package directories are
    /// relative to.
    pub cwd: PathBuf,
    /// The cache directory (`$BRIM_HOME`).
    pub home: PathBuf,
    /// Ignore the lock file and use the newest commit of every git
    /// dependency and the newest version of every registry dependency
    /// (`brim update`).
    pub update: bool,
    /// The registry's URL; `None` for [`registry::default_url`].
    pub registry: Option<String>,
}

/// Why resolving failed: a diagnostic and the manifest or lock file it
/// points into, if any.
#[derive(Debug)]
pub struct Failure {
    pub diagnostic: Box<Diagnostic>,
    pub file: Option<SourceFile>,
}

impl Failure {
    fn new(diagnostic: Diagnostic) -> Failure {
        Failure {
            diagnostic: Box::new(diagnostic),
            file: None,
        }
    }

    fn in_file(diagnostic: Diagnostic, file: &SourceFile) -> Failure {
        Failure {
            diagnostic: Box::new(diagnostic),
            file: Some(file.clone()),
        }
    }
}

/// What resolving did, besides the graph.
#[derive(Debug, Default)]
pub struct Report {
    /// The lock file was created or changed.
    pub lock_written: bool,
    /// Git repositories that were downloaded or updated, and registry
    /// packages that were downloaded (`name version`).
    pub fetched: Vec<String>,
}

/// The default cache directory: `$BRIM_HOME`, or `.brim` in the home
/// directory.
pub fn default_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("BRIM_HOME") {
        return Some(PathBuf::from(home));
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".brim"))
}

/// Resolves the dependencies of the project in `dir`, whose manifest is
/// `manifest`. Updates the lock file when the dependencies changed.
pub fn resolve(
    dir: &Path,
    manifest: Manifest,
    manifest_file: SourceFile,
    options: &Options,
) -> Result<(Graph, Report), Failure> {
    let lock_path = dir.join(LOCK_FILE);
    let locked = if options.update {
        lock::LockFile::default()
    } else {
        lock::read(&options.cwd.join(&lock_path), &lock_path)?
    };
    let mut resolver = Resolver {
        options,
        locked: &locked,
        packages: vec![Package {
            dir: dir.to_path_buf(),
            manifest,
            manifest_file,
            dependencies: Vec::new(),
            git: None,
            registry: None,
        }],
        by_name: HashMap::new(),
        report: Report::default(),
    };
    let root_name = resolver.packages[0].name().to_string();
    resolver.by_name.insert(root_name, (0, Identity::Root));
    // The project, path and git packages first; registry packages only
    // depend on registry packages, so they are added afterwards.
    let mut wanted = Vec::new();
    let mut next = 0;
    while next < resolver.packages.len() {
        let deps = resolver.packages[next].manifest.dependencies.clone();
        for dep in &deps {
            if let Source::Registry(req) = &dep.source {
                wanted.push(solve::Wanted {
                    from: next,
                    name: dep.name.clone(),
                    req: req.clone(),
                    span: dep.span,
                });
                continue;
            }
            let index = resolver.add(next, dep)?;
            resolver.packages[next]
                .dependencies
                .push((dep.name.clone(), index));
        }
        next += 1;
    }
    if !wanted.is_empty() {
        resolver.add_registry(&wanted)?;
    }
    let mut report = resolver.report;
    let graph = Graph {
        packages: resolver.packages,
    };
    if graph.packages.len() > 1 {
        let text = lock::render(&graph);
        let full = options.cwd.join(&lock_path);
        if std::fs::read_to_string(&full).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&full, text).map_err(|e| {
                Failure::new(Diagnostic::error(format!(
                    "cannot write {}: {e}",
                    slash(&lock_path)
                )))
            })?;
            report.lock_written = true;
        }
    }
    Ok((graph, report))
}

/// What makes two dependencies the same package.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Identity {
    Root,
    Path(PathBuf),
    Git { url: String, commit: String },
    Registry(Version),
}

struct Resolver<'a> {
    options: &'a Options,
    locked: &'a lock::LockFile,
    packages: Vec<Package>,
    by_name: HashMap<String, (usize, Identity)>,
    report: Report,
}

impl Resolver<'_> {
    /// Finds or adds the package `dep` of package `from` refers to.
    fn add(&mut self, from: usize, dep: &Dependency) -> Result<usize, Failure> {
        let from_pkg = &self.packages[from];
        let from_file = from_pkg.manifest_file.clone();
        let (dir, identity, git) = match &dep.source {
            Source::Registry(_) => unreachable!("registry dependencies are added by add_registry"),
            Source::Path(path) => {
                let dir = normalize(&from_pkg.dir.join(path));
                let full = self.options.cwd.join(&dir);
                let identity = Identity::Path(full.canonicalize().unwrap_or(full));
                (dir, identity, None)
            }
            Source::Git { url, reference } => {
                let locked = self
                    .locked
                    .commit(&dep.name, &lock::git_source(url, reference));
                let checkout = git::checkout(
                    &self.options.home,
                    &dep.name,
                    url,
                    reference,
                    locked,
                    &mut self.report.fetched,
                )
                .map_err(|message| {
                    Failure::in_file(
                        Diagnostic::error(format!("cannot get dependency `{}`", dep.name))
                            .with_code("M0008")
                            .with_primary(dep.span, "")
                            .with_note(message),
                        &from_file,
                    )
                })?;
                let identity = Identity::Git {
                    url: url.clone(),
                    commit: checkout.commit.clone(),
                };
                (checkout.dir, identity, Some((url.clone(), checkout.commit)))
            }
        };
        if let Some((index, existing)) = self.by_name.get(&dep.name) {
            if *existing == identity {
                return Ok(*index);
            }
            return Err(Failure::in_file(
                Diagnostic::error(format!("two different packages are named `{}`", dep.name))
                    .with_code("M0009")
                    .with_primary(dep.span, "")
                    .with_note(format!(
                        "`{}` is also {}",
                        dep.name,
                        describe(&self.packages[*index])
                    ))
                    .with_note("every package of a project needs its own name"),
                &from_file,
            ));
        }

        let manifest_path = dir.join(brim_manifest::MANIFEST_FILE);
        let shown = slash(&manifest_path);
        let text =
            std::fs::read_to_string(self.options.cwd.join(&manifest_path)).map_err(|_| {
                Failure::in_file(
                    Diagnostic::error(format!("cannot find package `{}`", dep.name))
                        .with_code("M0008")
                        .with_primary(dep.span, "")
                        .with_note(format!("there is no readable `{shown}`")),
                    &from_file,
                )
            })?;
        let manifest_file = SourceFile::new(shown, text);
        let manifest = brim_manifest::parse(manifest_file.text()).map_err(|diags| {
            Failure::in_file(diags.into_iter().next().expect("errors"), &manifest_file)
        })?;
        if manifest.package.name != dep.name {
            return Err(Failure::in_file(
                Diagnostic::error(format!(
                    "dependency `{}` is the package `{}`",
                    dep.name, manifest.package.name
                ))
                .with_code("M0009")
                .with_primary(dep.span, "")
                .with_help(format!(
                    "name the dependency `{}`; a dependency's name is its package's name",
                    manifest.package.name
                )),
                &from_file,
            ));
        }
        let index = self.packages.len();
        self.packages.push(Package {
            dir,
            manifest,
            manifest_file,
            dependencies: Vec::new(),
            git,
            registry: None,
        });
        self.by_name.insert(dep.name.clone(), (index, identity));
        Ok(index)
    }

    /// Picks, downloads and adds the registry packages `wanted` needs, and
    /// links them to the packages that depend on them.
    fn add_registry(&mut self, wanted: &[solve::Wanted]) -> Result<(), Failure> {
        let url = self
            .options
            .registry
            .clone()
            .unwrap_or_else(registry::default_url);
        let reg = registry::Registry::new(&url);
        for w in wanted {
            if let Some((index, _)) = self.by_name.get(&w.name) {
                let file = self.packages[w.from].manifest_file.clone();
                return Err(Failure::in_file(
                    Diagnostic::error(format!("two different packages are named `{}`", w.name))
                        .with_code("M0009")
                        .with_primary(w.span, "")
                        .with_note(format!(
                            "`{}` is also {}",
                            w.name,
                            describe(&self.packages[*index])
                        ))
                        .with_note("every package of a project needs its own name"),
                    &file,
                ));
            }
        }
        let chosen = solve::solve(&reg, self.options, self.locked, &self.packages, wanted)?;
        for (name, pick) in &chosen {
            let (dir, downloaded) = reg
                .source(&self.options.home, name, &pick.version, &pick.checksum)
                .map_err(|message| {
                    Failure::new(
                        Diagnostic::error(format!(
                            "cannot get dependency `{name}` {}",
                            pick.version
                        ))
                        .with_code("M0008")
                        .with_note(message),
                    )
                })?;
            if downloaded {
                self.report.fetched.push(format!("{name} {}", pick.version));
            }
            let manifest_path = dir.join(brim_manifest::MANIFEST_FILE);
            let shown = slash(&manifest_path);
            let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
                Failure::new(Diagnostic::error(format!("cannot read {shown}: {e}")))
            })?;
            let manifest_file = SourceFile::new(shown, text);
            let manifest = brim_manifest::parse(manifest_file.text()).map_err(|diags| {
                Failure::in_file(diags.into_iter().next().expect("errors"), &manifest_file)
            })?;
            if manifest.package.name != *name
                || manifest.package.version != pick.version.to_string()
            {
                return Err(Failure::in_file(
                    Diagnostic::error(format!(
                        "the registry's {name} {} contains the package {} {}",
                        pick.version, manifest.package.name, manifest.package.version
                    ))
                    .with_code("M0013"),
                    &manifest_file,
                ));
            }
            let index = self.packages.len();
            self.packages.push(Package {
                dir,
                manifest,
                manifest_file,
                dependencies: Vec::new(),
                git: None,
                registry: Some((url.clone(), pick.checksum.clone())),
            });
            self.by_name.insert(
                name.clone(),
                (index, Identity::Registry(pick.version.clone())),
            );
        }
        for w in wanted {
            let index = self.by_name[&w.name].0;
            self.packages[w.from]
                .dependencies
                .push((w.name.clone(), index));
        }
        for from in 0..self.packages.len() {
            if self.packages[from].registry.is_none() {
                continue;
            }
            let deps = self.packages[from].manifest.dependencies.clone();
            for dep in deps {
                let Some(&(index, _)) = self.by_name.get(&dep.name) else {
                    return Err(Failure::in_file(
                        Diagnostic::error(format!(
                            "the registry package `{}` depends on `{}`, which is missing",
                            self.packages[from].name(),
                            dep.name
                        ))
                        .with_code("M0013")
                        .with_primary(dep.span, ""),
                        &self.packages[from].manifest_file,
                    ));
                };
                self.packages[from].dependencies.push((dep.name, index));
            }
        }
        Ok(())
    }
}

fn describe(package: &Package) -> String {
    match (&package.git, &package.registry) {
        (Some((url, commit)), _) => format!("the git repository {url} at {commit}"),
        (_, Some(_)) => format!(
            "the registry package {} {}",
            package.name(),
            package.manifest.package.version
        ),
        _ => format!("the package in {}", slash(&package.dir)),
    }
}

/// A path as shown in messages: with `/` on every platform, like the
/// compiler's other paths.
fn slash(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Removes `.` and resolvable `..` components without touching the file
/// system, so paths stay readable in messages.
fn normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.last(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

#[cfg(test)]
mod tests;
