//! Loading a program: the root file, and the modules it imports from its
//! own package and from dependencies. Shared by the `brim` command and the
//! language server, which reads open files from the editor instead of the
//! disk.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use brim_diagnostics::{Diagnostic, SourceFile, SourceMap, Span};
use brim_semantic::ModuleAst;

/// The standard library modules, written in Brim and built into the
/// compiler: `use @std/net`.
pub const STD_MODULES: &[(&str, &str)] = &[
    ("std.base64", include_str!("../std/base64.brim")),
    ("std.crypto", include_str!("../std/crypto.brim")),
    ("std.fs", include_str!("../std/fs.brim")),
    ("std.http", include_str!("../std/http.brim")),
    ("std.json", include_str!("../std/json.brim")),
    ("std.log", include_str!("../std/log.brim")),
    ("std.net", include_str!("../std/net.brim")),
    ("std.postgres", include_str!("../std/postgres.brim")),
    ("std.prelude", include_str!("../std/prelude.brim")),
    ("std.process", include_str!("../std/process.brim")),
    ("std.random", include_str!("../std/random.brim")),
    ("std.task", include_str!("../std/task.brim")),
    ("std.time", include_str!("../std/time.brim")),
];

/// The module every program has: Brim functions behind built-in methods.
pub const PRELUDE: &str = "std.prelude";

/// The source of a standard library module.
pub fn std_module(name: &str) -> Option<&'static str> {
    STD_MODULES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}

/// A parsed program, ready for semantic analysis.
#[derive(Debug)]
pub struct Loaded {
    /// The root module first.
    pub modules: Vec<ModuleAst>,
    /// Every file, the root file first.
    pub map: SourceMap,
    /// Syntax errors and modules that cannot be found, in source order.
    pub diags: Vec<Diagnostic>,
}

/// Parses `root` and finds and parses the modules it imports, directly or
/// through other modules. In the root package, `use a/b` refers to
/// `a/b.brim` in the directory of the root file. `use @std/json` is a module
/// of the standard library, `use @dep` the library root of a dependency,
/// and `use @dep/a/b` the file `a/b.brim` next to it. The old spelling,
/// `import a.b`, names the standard library and dependencies without `@`;
/// the root package gets a warning for it (`W0005`).
///
/// Imports are rewritten to the full module names: inside the package
/// `geometry`, `use shapes` becomes the module `geometry.shapes`.
///
/// `read` reads a file, given its path as shown in messages (with `/`).
pub fn load(
    root: &SourceFile,
    graph: Option<&brim_package::Graph>,
    read: &mut dyn FnMut(&str) -> std::io::Result<Vec<u8>>,
) -> Loaded {
    let mut map = SourceMap::new();
    map.add(root.clone());
    let (mut program, mut diags) = brim_parser::parse_at(root.text(), 0);
    // Paths are shown with `/` on every platform, like the root file's.
    let name = root.name();
    let root_dir = match name.rfind(['/', '\\']) {
        Some(i) => name[..=i].to_string(),
        None => String::new(),
    };
    let mut pending = VecDeque::new();
    pending.extend(locate_imports(
        &mut program,
        0,
        graph,
        &root_dir,
        &mut diags,
    ));
    // Every program has the prelude, behind methods such as `xs.sorted()`.
    pending.push_back(ModuleFile {
        name: PRELUDE.to_string(),
        shown: "std/prelude.brim".to_string(),
        span: Span::point(0),
        package: 0,
    });
    let mut modules = vec![ModuleAst {
        name: String::new(),
        program,
    }];
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(target) = pending.pop_front() {
        let ModuleFile {
            name,
            shown,
            span,
            package,
        } = target;
        if !seen.insert(name.clone()) {
            continue;
        }
        let source = match std_module(&name) {
            Some(text) => Ok(text.as_bytes().to_vec()),
            None if name.starts_with("std.") || name == "std" => {
                diags.push(
                    Diagnostic::error(format!(
                        "the standard library has no module `@{}`",
                        name.replace('.', "/")
                    ))
                    .with_code("E0242")
                    .with_primary(span, "")
                    .with_note(format!(
                        "its modules are {}",
                        brim_semantic::STD_LIBRARY
                            .iter()
                            .map(|n| format!("`@std/{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                );
                continue;
            }
            None => read(&shown),
        };
        let text = match source.map(String::from_utf8) {
            Ok(Ok(text)) => text,
            Ok(Err(_)) => {
                diags.push(
                    Diagnostic::error(format!("module `{name}` is not valid UTF-8"))
                        .with_code("E0242")
                        .with_primary(span, "")
                        .with_note(format!("in `{shown}`")),
                );
                continue;
            }
            Err(_) => {
                diags.push(
                    Diagnostic::error(format!("cannot find module `{name}`"))
                        .with_code("E0242")
                        .with_primary(span, "")
                        .with_note(format!("looked for `{shown}`")),
                );
                continue;
            }
        };
        let base = map.add(SourceFile::new(shown, text));
        let file = &map.files()[map.files().len() - 1];
        let (mut program, module_diags) = brim_parser::parse_at(file.text(), base);
        diags.extend(module_diags);
        pending.extend(locate_imports(
            &mut program,
            package,
            graph,
            &root_dir,
            &mut diags,
        ));
        modules.push(ModuleAst { name, program });
    }
    diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
    Loaded {
        modules,
        map,
        diags,
    }
}

/// A module to load.
struct ModuleFile {
    /// The full module name.
    name: String,
    /// The file, as shown in messages.
    shown: String,
    /// The import that needs it.
    span: Span,
    /// The package it belongs to (an index into the graph; 0 without one).
    package: usize,
}

/// Finds the files of the modules `program` imports, a module of package
/// `package`, and rewrites its imports to full module names. Imports that
/// name no package are reported and left out.
fn locate_imports(
    program: &mut brim_ast::Program,
    package: usize,
    graph: Option<&brim_package::Graph>,
    root_dir: &str,
    diags: &mut Vec<Diagnostic>,
) -> Vec<ModuleFile> {
    let slash = |p: PathBuf| p.display().to_string().replace('\\', "/");
    let dependency = |name: &str| graph.and_then(|g| g.packages[package].dependency(name));
    let mut files = Vec::new();
    let mut unknown = Vec::new();
    for (index, import) in program.imports.iter_mut().enumerate() {
        let parts: Vec<String> = import.path.iter().map(|p| p.name.clone()).collect();
        let first = parts[0].as_str();
        let external = if import.legacy {
            let external = first == "std" || dependency(first).is_some();
            // Only the program's own package is asked to change; its
            // dependencies are someone else's code.
            if package == 0 {
                diags.push(
                    Diagnostic::warning("`import` is now written `use`")
                        .with_code("W0005")
                        .with_primary(import.span, "")
                        .with_help(format!(
                            "write `{}{}`",
                            if import.public { "pub " } else { "" },
                            import.use_text(external)
                        ))
                        .with_note("`brim fmt` rewrites every import"),
                );
            }
            external
        } else {
            import.external
        };
        if !external && (first == "std" || dependency(first).is_some()) {
            let what = if first == "std" {
                "the standard library".to_string()
            } else {
                format!("the dependency `{first}`")
            };
            diags.push(
                Diagnostic::error(format!("`{first}` is {what}"))
                    .with_code("E0242")
                    .with_primary(import.path[0].span, "")
                    .with_help(format!(
                        "packages are written with `@`: `{}`",
                        import.use_text(true)
                    )),
            );
        }
        // Read on as if it had the `@`, to avoid follow-up errors.
        let external = external || first == "std" || dependency(first).is_some();
        if external && first != "std" && dependency(first).is_none() {
            diags.push(
                Diagnostic::error(format!("there is no package `@{first}`"))
                    .with_code("E0242")
                    .with_primary(import.path[0].span, "")
                    .with_help(format!(
                        "add `{first}` to [dependencies] in {}, or write `@std` for the standard library",
                        brim_manifest::MANIFEST_FILE
                    )),
            );
            unknown.push(index);
            continue;
        }
        let mut target = 0;
        let mut prefix = None;
        let shown = match graph {
            // The standard library, built into the compiler.
            _ if external && first == "std" => format!("{}.brim", parts.join("/")),
            Some(g) if external => {
                target = dependency(first).unwrap_or_default();
                let dep = &g.packages[target];
                if parts.len() == 1 {
                    slash(dep.lib_file())
                } else {
                    format!("{}/{}.brim", slash(dep.lib_dir()), parts[1..].join("/"))
                }
            }
            Some(g) if package != 0 => {
                target = package;
                let own = &g.packages[package];
                prefix = Some(own.name().to_string());
                format!("{}/{}.brim", slash(own.lib_dir()), parts.join("/"))
            }
            _ => format!("{root_dir}{}.brim", parts.join("/")),
        };
        if let Some(prefix) = prefix {
            let span = import.path[0].span;
            import
                .path
                .insert(0, brim_ast::Ident { name: prefix, span });
        }
        files.push(ModuleFile {
            name: module_name(import),
            shown,
            span: import.span,
            package: target,
        });
    }
    // Reported already: the checker need not look for these.
    for index in unknown.into_iter().rev() {
        program.imports.remove(index);
    }
    files
}

fn module_name(import: &brim_ast::ImportDecl) -> String {
    import
        .path
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_std_library_lists_agree() {
        let names: Vec<String> = STD_MODULES
            .iter()
            .filter(|(n, _)| *n != PRELUDE)
            .map(|(n, _)| n.trim_start_matches("std.").to_string())
            .collect();
        assert_eq!(names, brim_semantic::STD_LIBRARY);
    }
}
