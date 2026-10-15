//! Loading a program: the root file, and the modules it imports from its
//! own package and from dependencies. Shared by the `brim` command and the
//! language server, which reads open files from the editor instead of the
//! disk.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use brim_diagnostics::{Diagnostic, SourceFile, SourceMap, Span};
use brim_semantic::ModuleAst;

/// The standard library modules, written in Brim and built into the
/// compiler: `import std.net`.
pub const STD_MODULES: &[(&str, &str)] = &[
    ("std.fs", include_str!("../std/fs.brim")),
    ("std.http", include_str!("../std/http.brim")),
    ("std.json", include_str!("../std/json.brim")),
    ("std.log", include_str!("../std/log.brim")),
    ("std.net", include_str!("../std/net.brim")),
    ("std.prelude", include_str!("../std/prelude.brim")),
    ("std.random", include_str!("../std/random.brim")),
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
/// through other modules. In the root package, `import a.b` refers to
/// `a/b.brim` in the directory of the root file. `import dep` refers to the
/// library root of a dependency, and `import dep.a.b` to `a/b.brim` next to
/// it.
///
/// Imports are rewritten to the full module names: inside the package
/// `geometry`, `import shapes` becomes `import geometry.shapes`.
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
    pending.extend(locate_imports(&mut program, 0, graph, &root_dir));
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
                    Diagnostic::error(format!("the standard library has no module `{name}`"))
                        .with_code("E0242")
                        .with_primary(span, "")
                        .with_note(format!(
                            "its modules are {}",
                            STD_MODULES
                                .iter()
                                .filter(|(n, _)| *n != PRELUDE)
                                .map(|(n, _)| format!("`{n}`"))
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
        pending.extend(locate_imports(&mut program, package, graph, &root_dir));
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
/// `package`, and rewrites its imports to full module names.
fn locate_imports(
    program: &mut brim_ast::Program,
    package: usize,
    graph: Option<&brim_package::Graph>,
    root_dir: &str,
) -> Vec<ModuleFile> {
    let slash = |p: PathBuf| p.display().to_string().replace('\\', "/");
    let mut files = Vec::new();
    for import in &mut program.imports {
        let parts: Vec<String> = import.path.iter().map(|p| p.name.clone()).collect();
        let mut target = 0;
        let mut prefix = None;
        let shown = match graph {
            // The standard library, built into the compiler.
            _ if parts[0] == "std" => format!("{}.brim", parts.join("/")),
            Some(g) if g.packages[package].dependency(&parts[0]).is_some() => {
                target = g.packages[package]
                    .dependency(&parts[0])
                    .unwrap_or_default();
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
