//! The `brim` command-line tool.
//!
//! The library half of the crate contains everything except process setup,
//! so commands can be tested in-process: [`run`] takes the arguments and
//! explicit output streams and returns an exit code (see [`exit`]).

pub mod args;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use args::{Command, Emit};
use brim_diagnostics::{
    ColorChoice, Diagnostic, Renderer, Severity, SourceFile, SourceMap, Sources, Span,
};
use brim_ir::Module;

/// Process exit codes. Documented in `docs/error-handling.md`.
pub mod exit {
    /// Success.
    pub const SUCCESS: i32 = 0;
    /// The program has syntax or semantic errors.
    pub const COMPILE_ERROR: i32 = 1;
    /// The command line was invalid.
    pub const USAGE: i32 = 2;
    /// An input file or manifest is missing, unreadable or invalid.
    pub const INPUT: i32 = 3;
    /// The requested feature (e.g. native code generation) is not available.
    pub const UNAVAILABLE: i32 = 4;
    /// The compiler itself failed (a bug in Brim).
    pub const INTERNAL: i32 = 70;
    /// The Brim program stopped with a run-time error.
    pub const RUNTIME_ERROR: i32 = 101;
}

/// Everything a command needs from its environment.
pub struct Context<'a> {
    /// Directory relative paths are resolved against.
    pub cwd: PathBuf,
    /// Program output and `--emit` output.
    pub stdout: &'a mut dyn Write,
    /// Diagnostics and status messages; also the program's standard error.
    pub stderr: &'a mut dyn Write,
    /// The program's standard input.
    pub stdin: &'a mut dyn io::BufRead,
    /// Whether diagnostics on `stderr` are colored.
    pub color: ColorChoice,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("cwd", &self.cwd)
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

/// Runs the `brim` command line (`args` excludes the program name).
pub fn run(args: &[String], ctx: &mut Context<'_>) -> i32 {
    let command = match args::parse(args) {
        Ok(command) => command,
        Err(err) => {
            let _ = writeln!(ctx.stderr, "error: {err}");
            let hint = match err.command {
                Some(cmd) => format!("brim {cmd} --help"),
                None => "brim --help".to_string(),
            };
            let _ = writeln!(ctx.stderr, "\nFor more information, run `{hint}`.");
            return exit::USAGE;
        }
    };
    let result = match command {
        Command::Help(None) => write_out(ctx.stdout, args::MAIN_HELP),
        Command::Help(Some(cmd)) => write_out(ctx.stdout, args::command_help(&cmd)),
        Command::Version => write_out(ctx.stdout, &format!("brim {}\n", env!("CARGO_PKG_VERSION"))),
        Command::Check { input, emit } => check(input.as_deref(), emit, ctx),
        Command::Run { input, args } => run_program(input.as_deref(), args, ctx),
        Command::Build {
            input,
            output,
            release,
        } => build(input.as_deref(), output.as_deref(), release, ctx),
        Command::Test { input, filter } => test(input.as_deref(), filter.as_deref(), ctx),
        Command::New { path } => new_project(&path, ctx),
        Command::Fmt { paths, check } => fmt(&paths, check, ctx),
        Command::Update { input } => update(input.as_deref(), ctx),
    };
    match result {
        Ok(()) => exit::SUCCESS,
        Err(code) => code,
    }
}

type CmdResult = Result<(), i32>;

fn write_out(out: &mut dyn Write, text: &str) -> CmdResult {
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|_| exit::INTERNAL)
}

// ---- commands ---------------------------------------------------------

fn check(input: Option<&Path>, emit: Option<Emit>, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_input(input, ctx)?;
    let file = &input.file;
    compile(&input, emit, false, ctx)?;
    let _ = writeln!(ctx.stderr, "ok: no errors found in {}", file.name());
    Ok(())
}

fn run_program(input: Option<&Path>, args: Vec<String>, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_input(input, ctx)?;
    let (module, map, _) = compile(&input, None, false, ctx)?;
    let mut host = brim_interp::Host {
        out: ctx.stdout,
        err: ctx.stderr,
        input: ctx.stdin,
        args,
        cwd: ctx.cwd.clone(),
    };
    let result = brim_interp::run(&module, &mut host, brim_interp::Limits::default());
    match result {
        Ok(0) => Ok(()),
        Ok(code) => Err(code),
        Err(trap) => {
            let diag = trap_diagnostic(&trap);
            report(&[diag], &map, ctx);
            Err(exit::RUNTIME_ERROR)
        }
    }
}

fn test(input: Option<&Path>, filter: Option<&str>, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_input(input, ctx)?;
    let (module, map, tests) = compile(&input, None, true, ctx)?;
    // The tests of dependencies are theirs to run.
    let dependencies: Vec<&str> = input
        .packages
        .iter()
        .flat_map(|g| &g.packages[1..])
        .map(brim_package::Package::name)
        .collect();
    let tests: Vec<_> = tests
        .into_iter()
        .filter(|&t| {
            let name = &module.function(t).name;
            let first = name.split('.').next().unwrap_or_default();
            name == first || !dependencies.contains(&first)
        })
        .filter(|&t| filter.is_none_or(|f| module.function(t).name.contains(f)))
        .collect();
    let _ = writeln!(ctx.stdout, "\nrunning {}", brim_plural(tests.len(), "test"));
    let mut failures = Vec::new();
    for &t in &tests {
        let name = &module.function(t).name;
        let _ = write!(ctx.stdout, "test {name} ... ");
        let _ = ctx.stdout.flush();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut host = brim_interp::Host {
            out: &mut out,
            err: &mut err,
            input: &mut io::empty(),
            args: Vec::new(),
            cwd: ctx.cwd.clone(),
        };
        let result =
            brim_interp::run_function(&module, t, &mut host, brim_interp::Limits::default());
        let failure = match result {
            Ok(0) => None,
            Ok(code) => Some(Diagnostic::error(format!("the test called `exit({code})`"))),
            Err(trap) => Some(trap_diagnostic(&trap)),
        };
        match failure {
            None => {
                let _ = writeln!(ctx.stdout, "ok");
            }
            Some(diag) => {
                let _ = writeln!(ctx.stdout, "FAILED");
                failures.push((name.clone(), out, err, diag));
            }
        }
    }
    if !failures.is_empty() {
        let _ = writeln!(ctx.stdout, "\nfailures:");
        for (name, out, err, diag) in &failures {
            let _ = writeln!(ctx.stdout, "\n---- {name} ----");
            let _ = ctx.stdout.write_all(out);
            let _ = ctx.stdout.write_all(err);
            let _ = ctx.stdout.flush();
            report(std::slice::from_ref(diag), &map, ctx);
        }
    }
    let passed = tests.len() - failures.len();
    let _ = writeln!(
        ctx.stdout,
        "\ntest result: {}. {passed} passed; {} failed",
        if failures.is_empty() { "ok" } else { "FAILED" },
        failures.len()
    );
    let _ = ctx.stdout.flush();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(exit::RUNTIME_ERROR)
    }
}

fn brim_plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn new_project(path: &Path, ctx: &mut Context<'_>) -> CmdResult {
    let dir = ctx.cwd.join(path);
    let shown = path.display().to_string();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if !brim_manifest::valid_name(&name) {
        return Err(input_error(
            ctx,
            &shown,
            Diagnostic::error(format!("`{name}` cannot be a package name"))
                .with_note(brim_manifest::NAME_RULES)
                .with_help("the directory's name is the package name"),
        ));
    }
    if dir.exists() {
        return Err(input_error(
            ctx,
            &shown,
            Diagnostic::error("already exists").with_help("choose a path that does not exist yet"),
        ));
    }
    let files = [
        (
            brim_manifest::MANIFEST_FILE.to_string(),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"{}\"\n",
                brim_manifest::EDITIONS[brim_manifest::EDITIONS.len() - 1]
            ),
        ),
        (
            "src/main.brim".to_string(),
            "fn main() {\n    print(greeting(\"world\"))\n}\n\nfn greeting(name: string) -> string {\n    return \"Hello, \" + name + \"!\"\n}\n\nfn test_greeting() {\n    assert_eq(greeting(\"Brim\"), \"Hello, Brim!\")\n}\n".to_string(),
        ),
        (".gitignore".to_string(), "/target\n".to_string()),
    ];
    for (file, text) in files {
        let full = dir.join(&file);
        let written = full
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&full, text));
        if let Err(e) = written {
            let shown = path.join(&file).display().to_string();
            return Err(input_error(
                ctx,
                &shown,
                Diagnostic::error(format!("cannot write file: {e}")),
            ));
        }
    }
    let _ = writeln!(ctx.stderr, "created project `{name}` in {shown}");
    Ok(())
}

fn update(input: Option<&Path>, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_project(input, true, ctx)?;
    let Some(graph) = &input.packages else {
        let _ = writeln!(
            ctx.stderr,
            "error: `brim update` works on a project, not a single file\n  \
             = help: pass the project's directory or brim.toml"
        );
        return Err(exit::USAGE);
    };
    let count = graph.packages.len() - 1;
    let what = if count == 1 {
        "dependency"
    } else {
        "dependencies"
    };
    let _ = writeln!(ctx.stderr, "ok: {count} {what} up to date");
    Ok(())
}

fn fmt(paths: &[PathBuf], check: bool, ctx: &mut Context<'_>) -> CmdResult {
    if paths.iter().any(|p| p.as_os_str() == "-") {
        if paths.len() > 1 {
            let _ = writeln!(ctx.stderr, "error: `-` cannot be combined with other paths");
            return Err(exit::USAGE);
        }
        let mut text = String::new();
        if io::Read::read_to_string(ctx.stdin, &mut text).is_err() {
            let _ = writeln!(ctx.stderr, "error: standard input is not valid UTF-8");
            return Err(exit::INPUT);
        }
        let file = SourceFile::new("<stdin>", text);
        return match brim_fmt::format(file.text()) {
            Ok(formatted) if check => {
                if formatted == file.text() {
                    Ok(())
                } else {
                    let _ = writeln!(ctx.stdout, "<stdin>");
                    Err(exit::COMPILE_ERROR)
                }
            }
            Ok(formatted) => write_out(ctx.stdout, &formatted),
            Err(diags) => {
                let mut map = SourceMap::new();
                map.add(file);
                report(&diags, &map, ctx);
                Err(exit::COMPILE_ERROR)
            }
        };
    }
    let roots = if paths.is_empty() {
        match brim_manifest::find(&ctx.cwd) {
            Some(manifest) => {
                let dir = manifest.parent().unwrap_or(Path::new("")).to_path_buf();
                vec![
                    dir.strip_prefix(&ctx.cwd)
                        .map(Path::to_path_buf)
                        .unwrap_or(dir),
                ]
            }
            None => {
                let _ = writeln!(
                    ctx.stderr,
                    "error: no path given, and no {} found in this directory or its parents\n  \
                     = help: pass the files or directories to format, e.g. `brim fmt src`",
                    brim_manifest::MANIFEST_FILE
                );
                return Err(exit::USAGE);
            }
        }
    } else {
        paths.to_vec()
    };
    let mut files = Vec::new();
    for root in &roots {
        let full = ctx.cwd.join(root);
        if full.is_dir() {
            collect_sources(root, &ctx.cwd, &mut files);
        } else if full.is_file() {
            files.push(root.clone());
        } else {
            let shown = root.display().to_string();
            return Err(input_error(
                ctx,
                &shown,
                Diagnostic::error("file not found"),
            ));
        }
    }
    let mut result = Ok(());
    for path in files {
        let file = read_text(&path, ctx)?;
        match brim_fmt::format(file.text()) {
            Ok(formatted) if formatted == file.text() => {}
            Ok(_) if check => {
                let _ = writeln!(ctx.stdout, "{}", path.display());
                result = Err(exit::COMPILE_ERROR);
            }
            Ok(formatted) => {
                if let Err(e) = std::fs::write(ctx.cwd.join(&path), formatted) {
                    let shown = path.display().to_string();
                    return Err(input_error(
                        ctx,
                        &shown,
                        Diagnostic::error(format!("cannot write file: {e}")),
                    ));
                }
                let _ = writeln!(ctx.stderr, "formatted {}", path.display());
            }
            Err(diags) => {
                let mut map = SourceMap::new();
                map.add(file);
                report(&diags, &map, ctx);
                result = Err(exit::COMPILE_ERROR);
            }
        }
    }
    result
}

/// The `.brim` files in `dir` and its subdirectories, without `target` and
/// hidden directories, in a stable order.
fn collect_sources(dir: &Path, cwd: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(cwd.join(dir)) else {
        return;
    };
    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let text = name.to_string_lossy();
        if cwd.join(&path).is_dir() {
            if text != "target" && !text.starts_with('.') {
                collect_sources(&path, cwd, files);
            }
        } else if path.extension().is_some_and(|e| e == "brim") {
            files.push(path);
        }
    }
}

/// The diagnostic for a run-time error, with the call stack.
fn trap_diagnostic(trap: &brim_interp::Trap) -> Diagnostic {
    let mut diag = Diagnostic::error(trap.error.to_string()).with_code(trap.error.code());
    if let Some(span) = trap.span {
        diag = diag.with_primary(span, "");
    }
    for note in trap.error.notes() {
        diag = diag.with_note(note);
    }
    if trap.backtrace.len() > 1 {
        let mut calls: Vec<&str> = trap.backtrace.iter().map(String::as_str).collect();
        if calls.len() > 8 {
            let omitted = calls.len() - 6;
            let tail = calls.split_off(calls.len() - 3);
            calls.truncate(3);
            calls.push("...");
            calls.extend(tail);
            diag = diag.with_note(format!("{omitted} calls omitted from the call stack"));
        }
        diag = diag.with_note(format!("call stack: {}", calls.join(" -> ")));
    }
    diag
}

fn build(
    input: Option<&Path>,
    output: Option<&Path>,
    release: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let input = load_input(input, ctx)?;
    let (module, map, _) = compile(&input, None, false, ctx)?;
    let Input {
        file,
        default_output,
        ..
    } = input;
    let shown = output.map_or(default_output, Path::to_path_buf);
    let output = ctx.cwd.join(&shown);
    if let Some(dir) = output.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        let _ = writeln!(
            ctx.stderr,
            "error: cannot create the output directory {}: {e}",
            dir.display()
        );
        return Err(exit::INPUT);
    }
    let options = brim_codegen::BuildOptions {
        output,
        opt_level: if release {
            brim_codegen::OptLevel::Release
        } else {
            brim_codegen::OptLevel::Debug
        },
        source: map,
    };
    match brim_codegen::native_backend().build(&module, &options) {
        Ok(_) => {
            let _ = writeln!(ctx.stderr, "built {}", shown.display());
            Ok(())
        }
        Err(brim_codegen::CodegenError::BackendUnavailable { backend, reason }) => {
            let _ = writeln!(
                ctx.stderr,
                "error: cannot build a native executable: the {backend} backend is not available\n  \
                 = note: {reason}\n  \
                 = note: {} has no errors; rebuild `brim` with `--features llvm` for native builds \
                 (docs/development.md)\n  \
                 = help: run the program with the reference interpreter: `brim run {}`",
                file.name(),
                file.name()
            );
            Err(exit::UNAVAILABLE)
        }
        Err(err @ brim_codegen::CodegenError::LinkerNotFound { .. }) => {
            let _ = writeln!(
                ctx.stderr,
                "error: {err}\n  \
                 = help: install a C compiler (clang or gcc), or set `BRIM_CC` to one"
            );
            Err(exit::UNAVAILABLE)
        }
        Err(err @ brim_codegen::CodegenError::LinkFailed { .. }) => {
            let _ = writeln!(ctx.stderr, "error: {err}");
            Err(exit::INTERNAL)
        }
        Err(err) => {
            let _ = writeln!(ctx.stderr, "error: {}", err.to_string().trim_end());
            let _ = writeln!(
                ctx.stderr,
                "  = note: this is a bug in the Brim compiler; please report it"
            );
            Err(exit::INTERNAL)
        }
    }
}

// ---- pipeline ---------------------------------------------------------

/// Runs the frontend and lowering. Prints diagnostics (and `--emit` output).
/// With `tests`, the program is checked as for `brim test`, and the IR
/// functions of its tests are returned.
fn compile(
    input: &Input,
    emit: Option<Emit>,
    tests: bool,
    ctx: &mut Context<'_>,
) -> Result<(Module, SourceMap, Vec<brim_ir::FuncId>), i32> {
    let file = &input.file;
    if emit == Some(Emit::Tokens) {
        let (tokens, _) = brim_lexer::lex(file.text());
        for token in &tokens {
            let loc = file.location(token.span.start);
            let _ = writeln!(ctx.stdout, "{loc}\t{:?}", token.kind);
        }
    }
    let mut map = SourceMap::new();
    map.add(file.clone());
    let (root, mut diags) = brim_parser::parse_at(file.text(), 0);
    if emit == Some(Emit::Ast) && !brim_diagnostics::has_errors(&diags) {
        let _ = writeln!(ctx.stdout, "{root:#?}");
    }
    let modules = load_modules(root, input, &mut map, &mut diags, ctx);
    diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
    if brim_diagnostics::has_errors(&diags) {
        report(&diags, &map, ctx);
        return Err(exit::COMPILE_ERROR);
    }

    let (checked, semantic_diags) = if tests {
        brim_semantic::check_tests(&modules)
    } else {
        brim_semantic::check_modules(&modules)
    };
    diags.extend(semantic_diags);
    report(&diags, &map, ctx);
    let Some(checked) = checked else {
        return Err(exit::COMPILE_ERROR);
    };

    let (module, test_fns) = brim_lower::lower_tests(&checked);
    if let Err(errors) = brim_ir::validate(&module) {
        let _ = writeln!(
            ctx.stderr,
            "error: internal compiler error: lowering produced invalid IR"
        );
        for e in errors {
            let _ = writeln!(ctx.stderr, "  {e}");
        }
        let _ = writeln!(
            ctx.stderr,
            "  = note: this is a bug in the Brim compiler; please report it"
        );
        return Err(exit::INTERNAL);
    }
    if emit == Some(Emit::Ir) {
        let _ = write!(ctx.stdout, "{module}");
    }
    let _ = ctx.stdout.flush();
    Ok((module, map, test_fns))
}

/// Finds and parses the modules a program imports, directly or through
/// other modules. In the root package, `import a.b` refers to `a/b.brim` in
/// the directory of the root file. `import dep` refers to the library root
/// of a dependency, and `import dep.a.b` to `a/b.brim` next to it.
///
/// Imports are rewritten to the full module names: inside the package
/// `geometry`, `import shapes` becomes `import geometry.shapes`.
fn load_modules(
    mut root: brim_ast::Program,
    input: &Input,
    map: &mut SourceMap,
    diags: &mut Vec<Diagnostic>,
    ctx: &mut Context<'_>,
) -> Vec<brim_semantic::ModuleAst> {
    // Paths are shown with `/` on every platform, like the root file's.
    let name = input.file.name();
    let root_dir = match name.rfind(['/', '\\']) {
        Some(i) => name[..=i].to_string(),
        None => String::new(),
    };
    let graph = input.packages.as_ref();
    let mut pending = std::collections::VecDeque::new();
    pending.extend(locate_imports(&mut root, 0, graph, &root_dir));
    let mut modules = vec![brim_semantic::ModuleAst {
        name: String::new(),
        program: root,
    }];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        let text = match std::fs::read(ctx.cwd.join(&shown)).map(String::from_utf8) {
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
        let source = SourceFile::new(shown, text);
        let base = map.add(source);
        let file = &map.files()[map.files().len() - 1];
        let (mut program, module_diags) = brim_parser::parse_at(file.text(), base);
        diags.extend(module_diags);
        pending.extend(locate_imports(&mut program, package, graph, &root_dir));
        modules.push(brim_semantic::ModuleAst { name, program });
    }
    modules
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

/// Renders diagnostics followed by a one-line summary.
fn report(diags: &[Diagnostic], sources: &SourceMap, ctx: &mut Context<'_>) {
    let file = sources.main_file();
    if diags.is_empty() {
        return;
    }
    let renderer = Renderer::new(ctx.color);
    let rendered = renderer.render_all(diags, sources);
    let _ = write!(ctx.stderr, "{rendered}");
    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diags.len() - errors;
    let plural = |n: usize, word: &str| {
        if n == 1 {
            format!("1 {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    let compile_errors = diags
        .iter()
        .any(|d| d.severity == Severity::Error && !d.code.is_some_and(|c| c.0.starts_with('R')));
    let summary = if compile_errors {
        let mut s = format!(
            "error: could not compile {} due to {}",
            file.name(),
            plural(errors, "error")
        );
        if warnings > 0 {
            s.push_str(&format!("; {} emitted", plural(warnings, "warning")));
        }
        s
    } else if errors == 0 {
        format!(
            "warning: {} generated {}",
            file.name(),
            plural(warnings, "warning")
        )
    } else {
        return;
    };
    let _ = writeln!(ctx.stderr, "\n{summary}");
}

// ---- input files ------------------------------------------------------

/// The program a command works on.
struct Input {
    file: SourceFile,
    /// Where `brim build` writes the executable unless told otherwise: next
    /// to a single source file, or in the project's `target` directory.
    default_output: PathBuf,
    /// For a project: its packages.
    packages: Option<brim_package::Graph>,
}

/// Reports a problem with an input file (no source location available).
fn input_error(ctx: &mut Context<'_>, path: &str, diag: Diagnostic) -> i32 {
    let file = SourceFile::new(path, "");
    let _ = write!(
        ctx.stderr,
        "{}",
        Renderer::new(ctx.color).render(&diag, &file)
    );
    exit::INPUT
}

/// Turns the command's PATH argument into the source file to compile.
fn load_input(input: Option<&Path>, ctx: &mut Context<'_>) -> Result<Input, i32> {
    load_project(input, false, ctx)
}

/// [`load_input`]; `update` gets the newest commits of git dependencies.
fn load_project(input: Option<&Path>, update: bool, ctx: &mut Context<'_>) -> Result<Input, i32> {
    let manifest = match input {
        None => match brim_manifest::find(&ctx.cwd) {
            Some(found) => {
                let shown = found
                    .strip_prefix(&ctx.cwd)
                    .map(Path::to_path_buf)
                    .unwrap_or(found);
                Some(shown)
            }
            None => {
                let _ = writeln!(
                    ctx.stderr,
                    "error: no input given, and no {} found in this directory or its parents\n  \
                     = help: pass a `.brim` file, e.g. `brim run examples/hello.brim`",
                    brim_manifest::MANIFEST_FILE
                );
                return Err(exit::USAGE);
            }
        },
        Some(path) => {
            let full = ctx.cwd.join(path);
            if full.is_dir() {
                Some(path.join(brim_manifest::MANIFEST_FILE))
            } else if path
                .file_name()
                .is_some_and(|n| n == brim_manifest::MANIFEST_FILE)
            {
                Some(path.to_path_buf())
            } else if path.extension().is_some_and(|e| e == "brim") {
                return Ok(Input {
                    file: read_source(path, ctx, None)?,
                    default_output: path.with_extension(std::env::consts::EXE_EXTENSION),
                    packages: None,
                });
            } else if !full.exists() {
                let shown = path.display().to_string();
                return Err(input_error(
                    ctx,
                    &shown,
                    Diagnostic::error("file not found"),
                ));
            } else {
                let shown = path.display().to_string();
                return Err(input_error(
                    ctx,
                    &shown,
                    Diagnostic::error("not a Brim source file")
                        .with_help("Brim source files use the `.brim` extension")
                        .with_help("to build a project, pass its directory or brim.toml"),
                ));
            }
        }
    };
    let manifest_path = manifest.expect("all other cases returned");
    let manifest_file = read_text(&manifest_path, ctx)?;
    let manifest = match brim_manifest::parse(manifest_file.text()) {
        Ok(m) => m,
        Err(diags) => {
            let renderer = Renderer::new(ctx.color);
            for diag in &diags {
                let _ = writeln!(ctx.stderr, "{}", renderer.render(diag, &manifest_file));
            }
            return Err(exit::INPUT);
        }
    };
    let project_dir = manifest_path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let mut entry = project_dir.join(&manifest.build.entry);
    // A library without a program: check and test its library root.
    let lib = project_dir.join(&manifest.build.lib);
    if !ctx.cwd.join(&entry).exists() && ctx.cwd.join(&lib).is_file() {
        entry = lib;
    }
    let note = format!(
        "the entry file is set by `[build] entry` in {} (default `{}`)",
        manifest_path.display(),
        brim_manifest::DEFAULT_ENTRY
    );
    let default_output = project_dir
        .join("target")
        .join(&manifest.package.name)
        .with_extension(std::env::consts::EXE_EXTENSION);
    let graph = resolve_packages(&project_dir, manifest, manifest_file, update, ctx)?;
    Ok(Input {
        file: read_source(&entry, ctx, Some(note))?,
        default_output,
        packages: Some(graph),
    })
}

/// Finds a project's dependencies (see `brim-package`).
fn resolve_packages(
    project_dir: &Path,
    manifest: brim_manifest::Manifest,
    manifest_file: SourceFile,
    update: bool,
    ctx: &mut Context<'_>,
) -> Result<brim_package::Graph, i32> {
    let Some(home) = brim_package::default_home() else {
        let _ = writeln!(
            ctx.stderr,
            "error: cannot find a home directory for the package cache\n  \
             = help: set BRIM_HOME to a directory brim can use"
        );
        return Err(exit::INPUT);
    };
    let options = brim_package::Options {
        cwd: ctx.cwd.clone(),
        home,
        update,
    };
    match brim_package::resolve(project_dir, manifest, manifest_file, &options) {
        Ok((graph, report)) => {
            for url in &report.fetched {
                let _ = writeln!(ctx.stderr, "fetched {url}");
            }
            if report.lock_written {
                let lock = project_dir.join(brim_package::LOCK_FILE);
                let _ = writeln!(ctx.stderr, "updated {}", lock.display());
            }
            Ok(graph)
        }
        Err(failure) => {
            let file = failure
                .file
                .unwrap_or_else(|| SourceFile::new(brim_manifest::MANIFEST_FILE, ""));
            let rendered = Renderer::new(ctx.color).render(&failure.diagnostic, &file);
            let _ = write!(ctx.stderr, "{rendered}");
            Err(exit::INPUT)
        }
    }
}

fn read_source(
    path: &Path,
    ctx: &mut Context<'_>,
    note: Option<String>,
) -> Result<SourceFile, i32> {
    match read_text(path, ctx) {
        Ok(file) => Ok(file),
        Err(code) => {
            if let Some(note) = note {
                let _ = writeln!(ctx.stderr, "  = note: {note}");
            }
            Err(code)
        }
    }
}

fn read_text(path: &Path, ctx: &mut Context<'_>) -> Result<SourceFile, i32> {
    let shown = path.display().to_string();
    let bytes = match std::fs::read(ctx.cwd.join(path)) {
        Ok(bytes) => bytes,
        Err(e) => {
            let message = match e.kind() {
                io::ErrorKind::NotFound => "file not found".to_string(),
                io::ErrorKind::IsADirectory => "expected a file, found a directory".to_string(),
                _ => format!("cannot read file: {e}"),
            };
            return Err(input_error(ctx, &shown, Diagnostic::error(message)));
        }
    };
    match String::from_utf8(bytes) {
        Ok(text) => Ok(SourceFile::new(shown, text)),
        Err(e) => {
            let offset = e.utf8_error().valid_up_to();
            let text = String::from_utf8_lossy(e.as_bytes()).into_owned();
            let file = SourceFile::new(shown, text);
            // Point at the first invalid byte (lossy decoding keeps offsets
            // before it intact).
            let diag = Diagnostic::error("file is not valid UTF-8")
                .with_primary(Span::point(offset), "invalid byte sequence here")
                .with_note("Brim source files must be encoded as UTF-8");
            let _ = write!(
                ctx.stderr,
                "{}",
                Renderer::new(ctx.color).render(&diag, &file)
            );
            Err(exit::INPUT)
        }
    }
}
