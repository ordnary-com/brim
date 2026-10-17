//! Picking versions of registry packages.
//!
//! Every registry package gets one version: the newest non-yanked one that
//! meets every requirement on it, or the locked one while it still does.
//! Requirements come from the project's own packages (fixed) and from the
//! picked versions of other registry packages (which change as picks
//! change), so picking repeats until nothing changes.
//!
//! When the lock file covers everything and the locked versions are
//! unpacked already, their manifests are enough and the registry is not
//! asked at all, so locked builds work offline.

use std::collections::{BTreeMap, HashMap};

use brim_diagnostics::{Diagnostic, Span};
use brim_manifest::{Source, Version, VersionReq};

use crate::registry::{IndexVersion, Registry};
use crate::{Failure, Options, Package, lock};

/// A registry dependency of one of the project's own packages.
#[derive(Clone, Debug)]
pub(crate) struct Wanted {
    /// The index of the depending package.
    pub from: usize,
    pub name: String,
    pub req: VersionReq,
    /// The entry in the depending package's manifest.
    pub span: Span,
}

/// A picked version.
#[derive(Clone, Debug)]
pub(crate) struct Pick {
    pub version: Version,
    pub checksum: String,
    dependencies: Vec<(String, VersionReq)>,
}

/// Who requires what, for messages.
#[derive(Clone, Debug)]
struct Requirement {
    req: VersionReq,
    /// `app` or `router 1.2.0`.
    by: String,
    /// For the project's own packages: the entry and its package.
    at: Option<(Span, usize)>,
}

/// Picks a version for every registry package the project needs.
pub(crate) fn solve(
    registry: &Registry,
    options: &Options,
    locked: &lock::LockFile,
    packages: &[Package],
    wanted: &[Wanted],
) -> Result<BTreeMap<String, Pick>, Failure> {
    let source = lock::registry_source(&registry.url);
    let locked_version = |name: &str| -> Option<(Version, String)> {
        if options.update {
            return None;
        }
        locked
            .registry(name, &source)
            .map(|(v, checksum)| (v, checksum.to_string()))
    };
    if !options.update
        && let Some(picks) = from_lock(registry, options, wanted, &locked_version)
    {
        return Ok(picks);
    }

    let mut index: HashMap<String, Vec<IndexVersion>> = HashMap::new();
    let mut picks: BTreeMap<String, Pick> = BTreeMap::new();
    for _ in 0..100 {
        let requirements = requirements(packages, wanted, &picks);
        let mut changed = false;
        let mut reached = BTreeMap::new();
        for (name, reqs) in &requirements {
            let versions = versions(registry, &mut index, name, reqs, packages)?;
            let current = picks.get(name);
            if current.is_some_and(|p| reqs.iter().all(|r| r.req.matches(&p.version))) {
                reached.insert(name.clone(), current.expect("checked").clone());
                continue;
            }
            let pick = pick(name, versions, reqs, locked_version(name), packages)?;
            reached.insert(name.clone(), pick);
            changed = true;
        }
        if reached.len() != picks.len() {
            changed = true;
        }
        picks = reached;
        if !changed {
            return Ok(picks);
        }
    }
    Err(Failure::new(
        Diagnostic::error("cannot pick versions of the registry packages")
            .with_code("M0014")
            .with_note("the requirements keep changing the picks; pin a version with `=`"),
    ))
}

/// The picks of the lock file, when they meet every requirement and are
/// unpacked already.
fn from_lock(
    registry: &Registry,
    options: &Options,
    wanted: &[Wanted],
    locked_version: &dyn Fn(&str) -> Option<(Version, String)>,
) -> Option<BTreeMap<String, Pick>> {
    let mut picks: BTreeMap<String, Pick> = BTreeMap::new();
    let mut queue: Vec<(String, VersionReq)> = wanted
        .iter()
        .map(|w| (w.name.clone(), w.req.clone()))
        .collect();
    while let Some((name, req)) = queue.pop() {
        if let Some(pick) = picks.get(&name) {
            if !req.matches(&pick.version) {
                return None;
            }
            continue;
        }
        let (version, checksum) = locked_version(&name)?;
        if !req.matches(&version) {
            return None;
        }
        let dir = registry.source_dir(&options.home, &name, &version);
        let text = std::fs::read_to_string(dir.join(brim_manifest::MANIFEST_FILE)).ok()?;
        let manifest = brim_manifest::parse(&text).ok()?;
        let mut dependencies = Vec::new();
        for dep in manifest.dependencies {
            let Source::Registry(req) = dep.source else {
                return None;
            };
            queue.push((dep.name.clone(), req.clone()));
            dependencies.push((dep.name, req));
        }
        picks.insert(
            name,
            Pick {
                version,
                checksum,
                dependencies,
            },
        );
    }
    Some(picks)
}

/// The requirements on every registry package reachable from `wanted`
/// through `picks`.
fn requirements(
    packages: &[Package],
    wanted: &[Wanted],
    picks: &BTreeMap<String, Pick>,
) -> BTreeMap<String, Vec<Requirement>> {
    let mut out: BTreeMap<String, Vec<Requirement>> = BTreeMap::new();
    let mut queue: Vec<String> = Vec::new();
    for w in wanted {
        if !out.contains_key(&w.name) {
            queue.push(w.name.clone());
        }
        out.entry(w.name.clone()).or_default().push(Requirement {
            req: w.req.clone(),
            by: packages[w.from].name().to_string(),
            at: Some((w.span, w.from)),
        });
    }
    let mut seen: Vec<String> = Vec::new();
    while let Some(name) = queue.pop() {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        let Some(pick) = picks.get(&name) else {
            continue;
        };
        for (dep, req) in &pick.dependencies {
            if !out.contains_key(dep) {
                queue.push(dep.clone());
            }
            out.entry(dep.clone()).or_default().push(Requirement {
                req: req.clone(),
                by: format!("{name} {}", pick.version),
                at: None,
            });
        }
    }
    out
}

/// The index of `name`, fetched once.
fn versions<'a>(
    registry: &Registry,
    index: &'a mut HashMap<String, Vec<IndexVersion>>,
    name: &str,
    reqs: &[Requirement],
    packages: &[Package],
) -> Result<&'a [IndexVersion], Failure> {
    if !index.contains_key(name) {
        let versions = registry
            .index(name)
            .map_err(|message| {
                failure(
                    Diagnostic::error(format!("cannot get dependency `{name}`"))
                        .with_code("M0008")
                        .with_note(message),
                    reqs,
                    packages,
                )
            })?
            .ok_or_else(|| {
                failure(
                    Diagnostic::error(format!("the registry has no package `{name}`"))
                        .with_code("M0013")
                        .with_note(format!("the registry is {}", registry.url)),
                    reqs,
                    packages,
                )
            })?;
        index.insert(name.to_string(), versions);
    }
    Ok(&index[name])
}

/// The newest version meeting `reqs`, or the locked one if it does.
fn pick(
    name: &str,
    versions: &[IndexVersion],
    reqs: &[Requirement],
    locked: Option<(Version, String)>,
    packages: &[Package],
) -> Result<Pick, Failure> {
    let fits = |v: &IndexVersion| reqs.iter().all(|r| r.req.matches(&v.version));
    let to_pick = |v: &IndexVersion| Pick {
        version: v.version.clone(),
        checksum: v.checksum.clone(),
        dependencies: v.dependencies.clone(),
    };
    // A locked version stays even when it was yanked later.
    if let Some((version, _)) = &locked
        && let Some(v) = versions.iter().find(|v| v.version == *version && fits(v))
    {
        return Ok(to_pick(v));
    }
    if let Some(v) = versions.iter().rev().find(|v| !v.yanked && fits(v)) {
        return Ok(to_pick(v));
    }
    let mut diag = Diagnostic::error(format!("no version of `{name}` meets every requirement"))
        .with_code("M0014");
    for r in reqs {
        diag = diag.with_note(format!("{} requires `{name} {}`", r.by, r.req));
    }
    let available: Vec<String> = versions
        .iter()
        .filter(|v| !v.yanked)
        .map(|v| v.version.to_string())
        .collect();
    diag = diag.with_note(if available.is_empty() {
        "every version is yanked".to_string()
    } else {
        let start = available.len().saturating_sub(8);
        format!("available versions: {}", available[start..].join(", "))
    });
    Err(failure(diag, reqs, packages))
}

/// A failure that points at the first requirement in the project's own
/// manifests, if any.
fn failure(diag: Diagnostic, reqs: &[Requirement], packages: &[Package]) -> Failure {
    match reqs.iter().find_map(|r| r.at) {
        Some((span, from)) => {
            Failure::in_file(diag.with_primary(span, ""), &packages[from].manifest_file)
        }
        None => Failure::new(diag),
    }
}
