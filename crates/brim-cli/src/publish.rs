//! `brim publish`, `brim login` and `brim yank` (see `docs/registry.md`).

use std::path::{Path, PathBuf};

use brim_diagnostics::Diagnostic;
use brim_manifest::{Manifest, Source};
use brim_package::registry::{self, Registry};

use crate::{CmdResult, Context, Input, Target, exit, input_error, load_input};

/// The file with registry tokens, in `$BRIM_HOME`.
const CREDENTIALS_FILE: &str = "credentials.toml";

pub(crate) fn publish(target: &Target<'_>, dry_run: bool, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_input(target, ctx)?;
    let Some(graph) = &input.packages else {
        let _ = writeln!(
            ctx.stderr,
            "error: `brim publish` publishes a package, not a single file\n  \
             = help: create a package with `brim new`"
        );
        return Err(exit::USAGE);
    };
    let package = &graph.packages[0];
    let manifest = &package.manifest;
    let name = manifest.package.name.clone();
    let version = manifest.package.version.clone();
    let shown_manifest = package.manifest_file.name().to_string();

    let mut problems = Vec::new();
    if !brim_manifest::valid_import_name(&name) {
        problems.push(format!(
            "registry packages are imported by name, so `{name}` cannot contain `-`"
        ));
    }
    let mut dependencies = Vec::new();
    for dep in &manifest.dependencies {
        match (&dep.source, &dep.version) {
            (Source::Git { .. }, _) => problems.push(format!(
                "`{}` is a git dependency; published packages can only depend on registry \
                 packages",
                dep.name
            )),
            (_, None) => problems.push(format!(
                "`{}` needs a `version`, e.g. `{} = {{ path = \"...\", version = \"0.1\" }}`",
                dep.name, dep.name
            )),
            (Source::Path(_), Some(req)) => {
                // The local package should be one the requirement allows.
                if let Some(index) = package.dependency(&dep.name) {
                    let local = &graph.packages[index].manifest.package.version;
                    if brim_manifest::Version::parse(local).is_some_and(|v| !req.matches(&v)) {
                        problems.push(format!(
                            "`{}` requires version `{req}`, but the package at its path is \
                             {local}",
                            dep.name
                        ));
                    }
                }
                dependencies.push((dep.name.clone(), req.to_string()));
            }
            (Source::Registry(_), Some(req)) => {
                dependencies.push((dep.name.clone(), req.to_string()));
            }
        }
    }
    let lib = package.lib_file();
    if !ctx.cwd.join(&lib).is_file() {
        problems.push(format!(
            "there is no library root {}; other packages `import {name}` it",
            lib.display()
        ));
    }
    if !problems.is_empty() {
        for problem in &problems {
            let _ = writeln!(ctx.stderr, "error: {problem}");
        }
        let _ = writeln!(ctx.stderr, "  = note: in {shown_manifest}");
        return Err(exit::INPUT);
    }
    for (field, value) in [
        ("description", &manifest.package.description),
        ("license", &manifest.package.license),
    ] {
        if value.is_none() {
            let _ = writeln!(
                ctx.stderr,
                "warning: {shown_manifest} has no `{field}` in `[package]`"
            );
        }
    }

    // The library must compile on its own.
    let lib_input = Input {
        file: crate::read_text(&lib, ctx)?,
        default_output: PathBuf::new(),
        packages: input.packages.clone(),
        lib: true,
    };
    crate::analyze(&lib_input, None, true, ctx)?;

    let dir = package.dir.clone();
    let files = package_files(&ctx.cwd.join(&dir), manifest);
    // Ustar headers hold paths of up to 255 bytes, with the prefix.
    if let Some(long) = files
        .iter()
        .find(|f| f.len() + name.len() + version.len() + 2 > 250)
    {
        let _ = writeln!(
            ctx.stderr,
            "error: the path {long} is too long for the package archive"
        );
        return Err(exit::INPUT);
    }
    let published = published_manifest(manifest, &dependencies);
    let prefix = format!("{name}-{version}");
    let archive = match registry::pack(
        &ctx.cwd.join(&dir),
        &files,
        &[(brim_manifest::MANIFEST_FILE.into(), published.into_bytes())],
        &prefix,
    ) {
        Ok(archive) => archive,
        Err(e) => {
            let _ = writeln!(ctx.stderr, "error: {e}");
            return Err(exit::INPUT);
        }
    };
    if archive.len() > registry::MAX_ARCHIVE {
        let _ = writeln!(
            ctx.stderr,
            "error: the archive is {} KiB; the registry accepts at most {} KiB",
            archive.len() / 1024,
            registry::MAX_ARCHIVE / 1024
        );
        return Err(exit::INPUT);
    }
    let _ = writeln!(
        ctx.stderr,
        "packed {} ({} files, {} KiB)",
        prefix,
        files.len() + 1,
        archive.len().div_ceil(1024)
    );

    if dry_run {
        let out = input
            .default_output
            .parent()
            .map_or(PathBuf::from("target"), Path::to_path_buf)
            .join("package")
            .join(format!("{prefix}.tar.gz"));
        let full = ctx.cwd.join(&out);
        let written = full
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&full, &archive));
        if let Err(e) = written {
            let shown = out.display().to_string();
            return Err(input_error(
                ctx,
                &shown,
                Diagnostic::error(format!("cannot write file: {e}")),
            ));
        }
        let mut listed: Vec<&str> = files.iter().map(String::as_str).collect();
        listed.push(brim_manifest::MANIFEST_FILE);
        listed.sort_unstable();
        for file in listed {
            let _ = writeln!(ctx.stdout, "{file}");
        }
        let _ = writeln!(
            ctx.stderr,
            "wrote {} (dry run: nothing was uploaded)",
            out.display()
        );
        return Ok(());
    }

    let reg = Registry::new(&registry::default_url());
    let token = token_for(&reg, ctx)?;
    let checksum = registry::hex(&brim_runtime::crypto::sha256(&archive));
    let metadata = serde_json::json!({
        "name": name,
        "version": version,
        "edition": manifest.package.edition,
        "checksum": checksum,
        "dependencies": dependencies
            .iter()
            .map(|(n, r)| (n.clone(), serde_json::Value::from(r.as_str())))
            .collect::<serde_json::Map<_, _>>(),
        "description": manifest.package.description,
        "license": manifest.package.license,
        "repository": manifest.package.repository,
    });
    match reg.publish(&token, &metadata.to_string(), &archive) {
        Ok(()) => {
            let _ = writeln!(ctx.stderr, "published {name} {version} to {}", reg.url);
            Ok(())
        }
        Err(e) => {
            let _ = writeln!(ctx.stderr, "error: cannot publish {name} {version}: {e}");
            Err(exit::INPUT)
        }
    }
}

/// The files of a package to publish, relative to its directory with `/`:
/// the library's directory (and `src/`), and the README, LICENSE and
/// CHANGELOG files at the top. Hidden files and `target` are left out; the
/// manifest is added separately.
fn package_files(dir: &Path, manifest: &Manifest) -> Vec<String> {
    let mut files = Vec::new();
    let mut roots = vec![PathBuf::from("src")];
    if let Some(parent) = manifest.build.lib.parent()
        && !parent.as_os_str().is_empty()
        && !parent.starts_with("src")
    {
        roots.push(parent.to_path_buf());
    }
    for root in roots {
        collect(dir, &root, &mut files);
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let upper = name.to_ascii_uppercase();
            if entry.path().is_file()
                && ["README", "LICENSE", "CHANGELOG"]
                    .iter()
                    .any(|p| upper.starts_with(p))
            {
                files.push(name);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn collect(base: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(base.join(dir)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        let path = dir.join(&name);
        if entry.path().is_dir() {
            collect(base, &path, files);
        } else if entry.path().is_file() {
            files.push(path.display().to_string().replace('\\', "/"));
        }
    }
}

/// The manifest as published: the package, its library root and the
/// dependencies by version only.
fn published_manifest(manifest: &Manifest, dependencies: &[(String, String)]) -> String {
    let p = &manifest.package;
    let mut out = String::from(
        "# Written by `brim publish` from the package's own brim.toml.\n\n[package]\n",
    );
    out.push_str(&format!("name = {}\n", quote(&p.name)));
    out.push_str(&format!("version = {}\n", quote(&p.version)));
    out.push_str(&format!("edition = {}\n", quote(&p.edition)));
    for (key, value) in [
        ("description", &p.description),
        ("license", &p.license),
        ("repository", &p.repository),
    ] {
        if let Some(value) = value {
            out.push_str(&format!("{key} = {}\n", quote(value)));
        }
    }
    let lib = manifest.build.lib.display().to_string().replace('\\', "/");
    if lib != brim_manifest::DEFAULT_LIB {
        out.push_str(&format!("\n[build]\nlib = {}\n", quote(&lib)));
    }
    if !dependencies.is_empty() {
        out.push_str("\n[dependencies]\n");
        for (name, req) in dependencies {
            out.push_str(&format!("{name} = {}\n", quote(req)));
        }
    }
    out
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

pub(crate) fn login(token: Option<String>, ctx: &mut Context<'_>) -> CmdResult {
    let reg = Registry::new(&registry::default_url());
    let token = match token {
        Some(token) => token,
        None => {
            let _ = writeln!(
                ctx.stderr,
                "paste the API token for {} and press Enter:",
                reg.url
            );
            let mut line = String::new();
            if ctx.stdin.read_line(&mut line).is_err() {
                let _ = writeln!(ctx.stderr, "error: cannot read the token");
                return Err(exit::INPUT);
            }
            line.trim().to_string()
        }
    };
    if token.is_empty() {
        let _ = writeln!(ctx.stderr, "error: the token is empty");
        return Err(exit::USAGE);
    }
    let user = match reg.me(&token) {
        Ok(user) => user,
        Err(e) => {
            let _ = writeln!(
                ctx.stderr,
                "error: the registry does not accept the token: {e}"
            );
            return Err(exit::INPUT);
        }
    };
    let Some(path) = credentials_path() else {
        let _ = writeln!(
            ctx.stderr,
            "error: cannot find a home directory; set BRIM_HOME"
        );
        return Err(exit::INPUT);
    };
    let mut table = read_credentials(&path);
    let mut entry = toml::Table::new();
    entry.insert("token".into(), toml::Value::String(token));
    table.insert(reg.url.clone(), toml::Value::Table(entry));
    let text = format!(
        "# Registry tokens, written by `brim login`. Keep this file private.\n{}",
        render_credentials(&table)
    );
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, text));
    if let Err(e) = written {
        let _ = writeln!(ctx.stderr, "error: cannot write {}: {e}", path.display());
        return Err(exit::INPUT);
    }
    restrict(&path);
    let _ = writeln!(ctx.stderr, "logged in to {} as {user}", reg.url);
    Ok(())
}

pub(crate) fn yank(name: &str, version: &str, undo: bool, ctx: &mut Context<'_>) -> CmdResult {
    let reg = Registry::new(&registry::default_url());
    let token = token_for(&reg, ctx)?;
    match reg.yank(&token, name, version, undo) {
        Ok(()) => {
            let done = if undo { "unyanked" } else { "yanked" };
            let _ = writeln!(ctx.stderr, "{done} {name} {version}");
            Ok(())
        }
        Err(e) => {
            let _ = writeln!(ctx.stderr, "error: {e}");
            Err(exit::INPUT)
        }
    }
}

/// The token for `reg`: `BRIM_REGISTRY_TOKEN`, or the one `brim login`
/// stored.
fn token_for(reg: &Registry, ctx: &mut Context<'_>) -> Result<String, i32> {
    if let Ok(token) = std::env::var("BRIM_REGISTRY_TOKEN")
        && !token.is_empty()
    {
        return Ok(token);
    }
    let stored = credentials_path()
        .map(|path| read_credentials(&path))
        .and_then(|table| {
            table
                .get(&reg.url)?
                .get("token")?
                .as_str()
                .map(str::to_string)
        });
    stored.ok_or_else(|| {
        let _ = writeln!(
            ctx.stderr,
            "error: no token for the registry {}\n  \
             = help: run `brim login`, or set BRIM_REGISTRY_TOKEN",
            reg.url
        );
        exit::USAGE
    })
}

fn credentials_path() -> Option<PathBuf> {
    brim_package::default_home().map(|home| home.join(CREDENTIALS_FILE))
}

fn read_credentials(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

fn render_credentials(table: &toml::Table) -> String {
    let mut out = String::new();
    for (url, entry) in table {
        let Some(token) = entry.get("token").and_then(toml::Value::as_str) else {
            continue;
        };
        out.push_str(&format!("\n[{}]\ntoken = {}\n", quote(url), quote(token)));
    }
    out
}

/// Makes the credentials readable by the user only, where that exists.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_: &Path) {}
