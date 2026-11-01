//! `brim add`, `brim remove` and `brim search`.
//!
//! `.brimproj` is edited as text, line by line, so comments, blank lines and
//! the order of everything else stay as they were; the result is parsed
//! again before it is written.

use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use brim_manifest::{Version, VersionReq};
use brim_package::registry::{self, Registry};

use crate::args::AddSource;
use crate::{CmdResult, Context, Target, exit, load_inputs, read_manifest, ui};

pub(crate) fn add(
    specs: &[String],
    source: Option<&AddSource>,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let start = Instant::now();
    let (path, text, manifest) = project_manifest("add", ctx)?;
    let manifest_dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut edited = text.clone();
    let mut done = Vec::new();
    for spec in specs {
        let (name, requirement) = match spec.split_once('@') {
            Some((name, req)) => (name, Some(req)),
            None => (spec.as_str(), None),
        };
        if !brim_manifest::valid_name(name) {
            return Err(error(
                ctx,
                &format!("`{name}` cannot be a package name"),
                Some(brim_manifest::NAME_RULES),
                exit::USAGE,
            ));
        }
        if name == manifest.package.name {
            return Err(error(
                ctx,
                &format!("`{name}` is this package itself"),
                None,
                exit::USAGE,
            ));
        }
        if requirement.is_some() && source.is_some() {
            return Err(error(
                ctx,
                "`NAME@VERSION` is for registry packages",
                Some("for `--path` and `--git`, write the version in .brimproj"),
                exit::USAGE,
            ));
        }
        let (value, shown) = match source {
            Some(AddSource::Path(dir)) => path_value(name, dir, &manifest_dir, ctx)?,
            Some(AddSource::Git { url, reference }) => git_value(url, reference.as_ref()),
            None => registry_value(name, requirement, ctx)?,
        };
        let previous = manifest
            .dependencies
            .iter()
            .find(|d| d.name == name)
            .map(|_| ());
        edited = match set_dependency(&edited, name, &value) {
            Ok(text) => text,
            Err(message) => return Err(error(ctx, &message, None, exit::INPUT)),
        };
        done.push((name.to_string(), shown, previous.is_some()));
    }
    write_and_resolve(&path, &text, &edited, ctx)?;
    let ui = ui::Ui::new(ctx);
    let last = done.len();
    for (i, (name, shown, replaced)) in done.iter().enumerate() {
        let elapsed = (i + 1 == last).then(|| start.elapsed());
        let (plain, verb) = if *replaced {
            ("updated", "Updated")
        } else {
            ("added", "Added")
        };
        let detail = format!("{} {}", ui.brand(name), ui.dim(shown));
        ui::done(
            ctx,
            &format!("{plain} {name} {shown}"),
            verb,
            &detail,
            elapsed,
        );
    }
    if ui.on {
        let uses: Vec<String> = done.iter().map(|(n, _, _)| format!("use @{n}")).collect();
        let _ = writeln!(
            ctx.stderr,
            "  {}",
            ui.dim(&format!("import with: {}", uses.join(", ")))
        );
    }
    Ok(())
}

pub(crate) fn remove(names: &[String], ctx: &mut Context<'_>) -> CmdResult {
    let start = Instant::now();
    let (path, text, manifest) = project_manifest("remove", ctx)?;
    let mut edited = text.clone();
    for name in names {
        if !manifest.dependencies.iter().any(|d| &d.name == name) {
            let known: Vec<&str> = manifest
                .dependencies
                .iter()
                .map(|d| d.name.as_str())
                .collect();
            let help = if known.is_empty() {
                "the project has no dependencies".to_string()
            } else {
                format!("its dependencies are: {}", known.join(", "))
            };
            return Err(error(
                ctx,
                &format!(
                    "`{name}` is not a dependency of `{}`",
                    manifest.package.name
                ),
                Some(&help),
                exit::USAGE,
            ));
        }
        edited = remove_dependency(&edited, name).unwrap_or(edited);
    }
    write_and_resolve(&path, &text, &edited, ctx)?;
    let last = names.len();
    for (i, name) in names.iter().enumerate() {
        let elapsed = (i + 1 == last).then(|| start.elapsed());
        let detail = ui::Ui::new(ctx).brand(name);
        ui::done(ctx, &format!("removed {name}"), "Removed", &detail, elapsed);
    }
    Ok(())
}

pub(crate) fn search(query: &str, ctx: &mut Context<'_>) -> CmdResult {
    let reg = Registry::new(&registry::default_url());
    let spinner = ui::spin(ctx, "Searching the registry");
    let found = reg.search(query);
    drop(spinner);
    let found = match found {
        Ok(found) => found,
        Err(e) => {
            return Err(error(
                ctx,
                &format!("cannot search {}: {e}", reg.url),
                None,
                exit::INPUT,
            ));
        }
    };
    if found.is_empty() {
        let what = if query.is_empty() {
            "the registry has no packages yet".to_string()
        } else {
            format!("no packages match `{query}`")
        };
        let _ = writeln!(ctx.stderr, "{what}");
        return Ok(());
    }
    let ui = ui::Ui::stdout(ctx);
    let name_width = found.iter().map(|f| f.name.len()).max().unwrap_or(0);
    let version_width = found.iter().map(|f| f.version.len()).max().unwrap_or(0);
    let mut out = String::new();
    for f in &found {
        let description = f.description.as_deref().unwrap_or("").trim();
        let line = format!(
            "{}  {}  {}",
            ui.brand(&ui.bold(&format!("{:<name_width$}", f.name))),
            ui.dim(&format!("{:<version_width$}", f.version)),
            description
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    let _ = ctx.stdout.write_all(out.as_bytes());
    let _ = ctx.stdout.flush();
    if ui.on {
        let _ = writeln!(
            ctx.stderr,
            "\n{}",
            ui.dim(&format!(
                "{} · add one with `brim add NAME`",
                crate::brim_plural(found.len(), "package")
            ))
        );
    }
    Ok(())
}

// ---- steps ------------------------------------------------------------

/// The nearest `.brimproj` with a `[package]`: its path, text and package.
fn project_manifest(
    command: &str,
    ctx: &mut Context<'_>,
) -> Result<(PathBuf, String, brim_manifest::Manifest), i32> {
    let Some(path) = brim_manifest::find(&ctx.cwd) else {
        return Err(error(
            ctx,
            &format!(
                "no {} found in this directory or its parents",
                brim_manifest::MANIFEST_FILE
            ),
            Some(&format!(
                "run `brim {command}` inside a project, or create one with `brim new`"
            )),
            exit::USAGE,
        ));
    };
    let (file, root) = read_manifest(&path, ctx)?;
    let Some(manifest) = root.package else {
        return Err(error(
            ctx,
            &format!(
                "{} is a workspace without a package of its own",
                path.display()
            ),
            Some(&format!(
                "run `brim {command}` in the directory of a member"
            )),
            exit::USAGE,
        ));
    };
    Ok((path, file.text().to_string(), manifest))
}

/// The newest version of `name` that meets `requirement`, as the text for
/// `.brimproj` and for the output.
fn registry_value(
    name: &str,
    requirement: Option<&str>,
    ctx: &mut Context<'_>,
) -> Result<(String, String), i32> {
    let wanted = match requirement.map(VersionReq::parse) {
        Some(Ok(req)) => Some(req),
        Some(Err(e)) => {
            return Err(error(
                ctx,
                &format!("invalid version requirement for `{name}`: {e}"),
                Some("for example `router@1.2`, `router@^1.2.3` or `router@=1.2.3`"),
                exit::USAGE,
            ));
        }
        None => None,
    };
    let reg = Registry::new(&registry::default_url());
    let spinner = ui::spin(ctx, &format!("Looking up {name}"));
    let index = reg.index(name);
    drop(spinner);
    let versions = match index {
        Ok(Some(versions)) => versions,
        Ok(None) => {
            return Err(error(
                ctx,
                &format!("the registry has no package `{name}`"),
                Some(&format!("look for it with `brim search {name}`")),
                exit::INPUT,
            ));
        }
        Err(e) => {
            return Err(error(
                ctx,
                &format!("cannot look up `{name}` in {}: {e}", reg.url),
                None,
                exit::INPUT,
            ));
        }
    };
    let usable: Vec<&Version> = versions
        .iter()
        .filter(|v| !v.yanked)
        .map(|v| &v.version)
        .collect();
    let newest = match &wanted {
        Some(req) => usable.iter().rev().find(|v| req.matches(v)).copied(),
        None => usable
            .iter()
            .rev()
            .find(|v| !v.is_prerelease())
            .or(usable.last())
            .copied(),
    };
    let Some(newest) = newest else {
        let help = match usable.last() {
            Some(v) => format!("the newest version is {v}"),
            None => "every version of it is yanked".to_string(),
        };
        let message = match requirement {
            Some(req) => format!("no version of `{name}` meets `{req}`"),
            None => format!("`{name}` has no version to use"),
        };
        return Err(error(ctx, &message, Some(&help), exit::INPUT));
    };
    let text = match requirement {
        Some(req) => req.to_string(),
        None if newest.is_prerelease() => format!("={newest}"),
        None => format!("^{newest}"),
    };
    let shown = if requirement.is_some() {
        format!("{text} ({newest})")
    } else {
        text.clone()
    };
    Ok((quote(&text), shown))
}

fn path_value(
    name: &str,
    dir: &Path,
    manifest_dir: &Path,
    ctx: &mut Context<'_>,
) -> Result<(String, String), i32> {
    let full = ctx.cwd.join(dir);
    let theirs = brim_manifest::manifest_path(&full);
    let Ok(text) = std::fs::read_to_string(&theirs) else {
        return Err(error(
            ctx,
            &format!("{} has no {}", dir.display(), brim_manifest::MANIFEST_FILE),
            Some("`--path` takes the directory of a Brim package"),
            exit::INPUT,
        ));
    };
    let their_name = brim_manifest::parse_root(&text)
        .ok()
        .and_then(|root| root.package)
        .map(|m| m.package.name);
    match their_name {
        Some(n) if n == name => {}
        Some(n) => {
            return Err(error(
                ctx,
                &format!("the package in {} is `{n}`, not `{name}`", dir.display()),
                Some(&format!(
                    "a dependency has the package's own name: `brim add {n} --path {}`",
                    dir.display()
                )),
                exit::USAGE,
            ));
        }
        None => {
            return Err(error(
                ctx,
                &format!("{} has no `[package]`", theirs.display()),
                None,
                exit::INPUT,
            ));
        }
    }
    let manifest_dir = ctx.cwd.join(manifest_dir);
    let written = relative(&manifest_dir, &full).unwrap_or(full);
    let written = written.display().to_string().replace('\\', "/");
    Ok((
        format!("{{ path = {} }}", quote(&written)),
        format!("(path {written})"),
    ))
}

fn git_value(url: &str, reference: Option<&(&'static str, String)>) -> (String, String) {
    match reference {
        Some((kind, value)) => (
            format!("{{ git = {}, {kind} = {} }}", quote(url), quote(value)),
            format!("(git {url} {kind} {value})"),
        ),
        None => (
            format!("{{ git = {} }}", quote(url)),
            format!("(git {url})"),
        ),
    }
}

/// Writes the edited manifest and fetches and locks its dependencies; on
/// failure, puts the old manifest back.
fn write_and_resolve(path: &Path, old: &str, new: &str, ctx: &mut Context<'_>) -> CmdResult {
    if let Err(diags) = brim_manifest::parse_root(new) {
        // A bug in the editing below, never the user's input.
        let _ = writeln!(
            ctx.stderr,
            "error: internal error: the edited {} does not parse: {}\n  \
             = note: this is a bug in brim; please report it",
            brim_manifest::MANIFEST_FILE,
            diags.first().map_or(String::new(), |d| d.message.clone())
        );
        return Err(exit::INTERNAL);
    }
    let full = ctx.cwd.join(path);
    if let Err(e) = std::fs::write(&full, new) {
        return Err(error(
            ctx,
            &format!("cannot write {}: {e}", path.display()),
            None,
            exit::INPUT,
        ));
    }
    // Relative, so messages show `.brimlock` rather than a full path.
    let target_path = Some(relative(&ctx.cwd, &full).unwrap_or_else(|| path.to_path_buf()));
    let package = None;
    let resolved = load_inputs(&Target::new(&target_path, &package), false, ctx);
    if resolved.is_err() {
        let _ = std::fs::write(&full, old);
        let ui = ui::Ui::new(ctx);
        let _ = writeln!(
            ctx.stderr,
            "{}",
            ui.dim(&format!("{} is unchanged", brim_manifest::MANIFEST_FILE))
        );
    }
    resolved.map(|_| ())
}

/// `error: message` and an optional `= help:` line; returns `code`.
fn error(ctx: &mut Context<'_>, message: &str, help: Option<&str>, code: i32) -> i32 {
    ui::stop();
    let ui = ui::Ui::new(ctx);
    let _ = writeln!(ctx.stderr, "{} {message}", ui.red(&ui.bold("error:")));
    if let Some(help) = help {
        let _ = writeln!(ctx.stderr, "  {} {help}", ui.bold("= help:"));
    }
    code
}

/// `to` relative to `from`, both absolute or both relative to the same
/// directory.
fn relative(from: &Path, to: &Path) -> Option<PathBuf> {
    let parts = |p: &Path| -> Vec<_> {
        normalize(p)
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect()
    };
    let (from, to) = (parts(from), parts(to));
    if from.first() != to.first() {
        return None;
    }
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

/// Removes `.` and folds `..` into the component before it.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

// ---- editing .brimproj ------------------------------------------------

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The table a line opens (`dependencies` for `[dependencies]`), if it is
/// a table header.
fn table_header(line: &str) -> Option<String> {
    let line = line.split('#').next().unwrap_or("").trim();
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner
        .strip_prefix('[')
        .and_then(|i| i.strip_suffix(']'))
        .unwrap_or(inner);
    Some(
        inner
            .split('.')
            .map(|p| p.trim().trim_matches('"'))
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// The key a `key = value` line sets.
fn line_key(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    let key = key
        .strip_prefix('"')
        .and_then(|k| k.strip_suffix('"'))
        .unwrap_or(key);
    (!key.is_empty() && !key.starts_with('#')).then_some(key)
}

/// The line range of the `[dependencies]` table's body.
fn dependencies_body(lines: &[&str]) -> Option<(usize, usize)> {
    let header = lines
        .iter()
        .position(|l| table_header(l).as_deref() == Some("dependencies"))?;
    let end = lines[header + 1..]
        .iter()
        .position(|l| table_header(l).is_some())
        .map_or(lines.len(), |i| header + 1 + i);
    Some((header + 1, end))
}

fn join(lines: &[String], had_newline: bool) -> String {
    let mut out = lines.join("\n");
    if had_newline || !out.is_empty() {
        out.push('\n');
    }
    out
}

/// `text` with `name = value` in `[dependencies]`: replacing the entry for
/// `name`, else in name order among the others, adding the table when there
/// is none.
fn set_dependency(text: &str, name: &str, value: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let entry = format!("{name} = {value}");
    if lines
        .iter()
        .any(|l| table_header(l).as_deref() == Some(&format!("dependencies.{name}")))
    {
        return Err(format!(
            "`{name}` is written as a `[dependencies.{name}]` table; change it in {} by hand",
            brim_manifest::MANIFEST_FILE
        ));
    }
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    let Some((start, end)) = dependencies_body(&lines) else {
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push("[dependencies]".into());
        out.push(entry);
        return Ok(join(&out, true));
    };
    let keys: Vec<(usize, &str)> = (start..end)
        .filter_map(|i| line_key(lines[i]).map(|k| (i, k)))
        .collect();
    if let Some(&(i, _)) = keys.iter().find(|(_, k)| *k == name) {
        out[i] = entry;
        return Ok(join(&out, text.ends_with('\n')));
    }
    let sorted = keys.windows(2).all(|w| w[0].1 <= w[1].1);
    let at = match keys.iter().find(|(_, k)| sorted && *k > name) {
        Some(&(i, _)) => i,
        None => match keys.last() {
            Some(&(i, _)) => i + 1,
            // An empty table: after its last non-blank line.
            None => (start..end)
                .rev()
                .find(|&i| !lines[i].trim().is_empty())
                .map_or(start, |i| i + 1),
        },
    };
    out.insert(at, entry);
    Ok(join(&out, text.ends_with('\n')))
}

/// `text` without the entry for `name` in `[dependencies]` (or its
/// `[dependencies.name]` table); `None` when there is none.
fn remove_dependency(text: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    if let Some(header) = lines
        .iter()
        .position(|l| table_header(l).as_deref() == Some(&format!("dependencies.{name}")))
    {
        let end = lines[header + 1..]
            .iter()
            .position(|l| table_header(l).is_some())
            .map_or(lines.len(), |i| header + 1 + i);
        out.drain(header..end);
        return Some(join(&out, text.ends_with('\n')));
    }
    let (start, end) = dependencies_body(&lines)?;
    let i = (start..end).find(|&i| line_key(lines[i]) == Some(name))?;
    out.remove(i);
    Some(join(&out, text.ends_with('\n')))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\
# My project
[package]
name = \"app\"
version = \"0.1.0\"
edition = \"2026\"
";

    #[test]
    fn adds_the_table_when_missing() {
        let out = set_dependency(BASE, "router", "\"^1.2.0\"").unwrap();
        assert_eq!(
            out,
            format!("{BASE}\n[dependencies]\nrouter = \"^1.2.0\"\n")
        );
    }

    #[test]
    fn inserts_in_name_order_and_keeps_comments() {
        let text = format!(
            "{BASE}\n[dependencies]\n# web\nalpha = \"1\"\nzeta = \"2\" # pinned\n\n[build]\nlink = []\n"
        );
        let out = set_dependency(&text, "router", "\"^1.2.0\"").unwrap();
        assert_eq!(
            out,
            format!(
                "{BASE}\n[dependencies]\n# web\nalpha = \"1\"\nrouter = \"^1.2.0\"\nzeta = \"2\" # pinned\n\n[build]\nlink = []\n"
            )
        );
    }

    #[test]
    fn appends_when_unsorted() {
        let text = format!("{BASE}[dependencies]\nzeta = \"2\"\nalpha = \"1\"\n");
        let out = set_dependency(&text, "beta", "\"3\"").unwrap();
        assert!(out.ends_with("alpha = \"1\"\nbeta = \"3\"\n"));
    }

    #[test]
    fn replaces_an_existing_entry() {
        let text = format!("{BASE}[dependencies]\nrouter = \"1.0\"\n");
        let out = set_dependency(&text, "router", "{ path = \"../router\" }").unwrap();
        assert!(out.ends_with("[dependencies]\nrouter = { path = \"../router\" }\n"));
    }

    #[test]
    fn refuses_subtables() {
        let text = format!("{BASE}[dependencies.router]\nversion = \"1\"\n");
        assert!(set_dependency(&text, "router", "\"2\"").is_err());
    }

    #[test]
    fn removes_entries_and_subtables() {
        let text = format!("{BASE}[dependencies]\nalpha = \"1\"\nrouter = \"2\"\n");
        assert_eq!(
            remove_dependency(&text, "router").unwrap(),
            format!("{BASE}[dependencies]\nalpha = \"1\"\n")
        );
        let text = format!("{BASE}[dependencies.router]\nversion = \"1\"\n\n[build]\n");
        assert_eq!(
            remove_dependency(&text, "router").unwrap(),
            format!("{BASE}[build]\n")
        );
        assert_eq!(remove_dependency(BASE, "router"), None);
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative(Path::new("/a/app"), Path::new("/a/libs/geo")),
            Some(PathBuf::from("../libs/geo"))
        );
        assert_eq!(
            relative(Path::new("/a"), Path::new("/a/./b")),
            Some(PathBuf::from("b"))
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
