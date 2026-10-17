//! The `brim` command-line tool.
//!
//! The library half of the crate contains everything except process setup,
//! so commands can be tested in-process: [`run`] takes the arguments and
//! explicit output streams and returns an exit code (see [`exit`]).

pub mod args;
mod publish;

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
        Command::Version => {
            let backend = if brim_codegen::LlvmBackend::AVAILABLE {
                "native backend: LLVM 22"
            } else {
                "no native backend"
            };
            write_out(
                ctx.stdout,
                &format!("brim {} ({backend})\n", env!("CARGO_PKG_VERSION")),
            )
        }
        Command::Check {
            input,
            package,
            emit,
        } => check(&Target::new(&input, &package), emit, ctx),
        Command::Run {
            input,
            package,
            args,
        } => run_program(&Target::new(&input, &package), args, ctx),
        Command::Build {
            input,
            package,
            output,
            release,
        } => build(
            &Target::new(&input, &package),
            output.as_deref(),
            release,
            ctx,
        ),
        Command::Test {
            input,
            package,
            filter,
        } => test(&Target::new(&input, &package), filter.as_deref(), ctx),
        Command::New { path } => new_project(&path, ctx),
        Command::Fmt { paths, check } => fmt(&paths, check, ctx),
        Command::Update { input, package } => update(&Target::new(&input, &package), ctx),
        Command::Publish {
            input,
            package,
            dry_run,
        } => publish::publish(&Target::new(&input, &package), dry_run, ctx),
        Command::Login { token } => publish::login(token, ctx),
        Command::Yank {
            name,
            version,
            undo,
        } => publish::yank(&name, &version, undo, ctx),
        Command::Lsp => match brim_lsp::run(ctx.stdin, ctx.stdout) {
            Ok(0) => Ok(()),
            Ok(code) => Err(code),
            Err(e) => {
                let _ = writeln!(ctx.stderr, "error: language server: {e}");
                Err(exit::INTERNAL)
            }
        },
        Command::GenTs {
            input,
            package,
            output,
            angular,
        } => gen_ts(
            &Target::new(&input, &package),
            output.as_deref(),
            angular,
            ctx,
        ),
        Command::Doc {
            input,
            package,
            output,
            std,
        } => doc(&Target::new(&input, &package), output.as_deref(), std, ctx),
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

fn check(target: &Target<'_>, emit: Option<Emit>, ctx: &mut Context<'_>) -> CmdResult {
    let mut result = Ok(());
    for input in load_inputs(target, false, ctx)? {
        match compile(&input, emit, input.lib, ctx) {
            Ok(_) => {
                let _ = writeln!(ctx.stderr, "ok: no errors found in {}", input.file.name());
            }
            Err(code) => result = result.and(Err(code)),
        }
    }
    result
}

/// Runs a program given as text, as `brim run` would run a file of that
/// name, for embedders such as the online playground. Other modules than
/// `std` cannot be imported. `max_steps` stops runaway programs with the
/// run-time error `R0009`. Returns the exit code.
pub fn run_source(name: &str, text: &str, max_steps: Option<u64>, ctx: &mut Context<'_>) -> i32 {
    let input = Input {
        file: SourceFile::new(name, text),
        default_output: PathBuf::new(),
        packages: None,
        lib: false,
    };
    let result = compile(&input, None, false, ctx).and_then(|(module, map, _)| {
        let mut host = brim_interp::Host {
            out: ctx.stdout,
            err: ctx.stderr,
            input: ctx.stdin,
            args: Vec::new(),
            cwd: ctx.cwd.clone(),
        };
        let limits = brim_interp::Limits {
            max_steps,
            ..brim_interp::Limits::default()
        };
        match brim_interp::run(&module, &mut host, limits) {
            Ok(0) => Ok(()),
            Ok(code) => Err(code),
            Err(trap) => {
                let diag = trap_diagnostic(&trap);
                report(&[diag], &map, ctx);
                Err(exit::RUNTIME_ERROR)
            }
        }
    });
    let _ = ctx.stdout.flush();
    match result {
        Ok(()) => exit::SUCCESS,
        Err(code) => code,
    }
}

fn run_program(target: &Target<'_>, args: Vec<String>, ctx: &mut Context<'_>) -> CmdResult {
    let input = load_input(target, ctx)?;
    let (module, map, _) = compile(&input, None, false, ctx)?;
    // The interpreter cannot call C: build programs with `extern fn`s and
    // run the executable instead.
    let calls_c = module
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.insts)
        .any(|i| matches!(i.kind, brim_ir::InstKind::CallC(..)));
    if calls_c && brim_codegen::LlvmBackend::AVAILABLE {
        return run_native(input, module, map, args, ctx);
    }
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

/// `brim run` of a program that calls C: builds it like `brim build` and
/// runs the executable, with the program's arguments.
fn run_native(
    input: Input,
    module: Module,
    map: SourceMap,
    args: Vec<String>,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let shown = input.default_output.clone();
    built_executable(input, module, map, None, false, ctx)?;
    let mut child = match std::process::Command::new(ctx.cwd.join(&shown))
        .args(&args)
        .current_dir(&ctx.cwd)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = writeln!(ctx.stderr, "error: cannot run {}: {e}", shown.display());
            return Err(exit::INPUT);
        }
    };
    // Standard error on a thread, standard output as it comes.
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = io::Read::read_to_end(&mut stderr, &mut buffer);
        buffer
    });
    if let Some(mut stdout) = child.stdout.take() {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = io::Read::read(&mut stdout, &mut chunk) {
            if n == 0 {
                break;
            }
            let _ = ctx.stdout.write_all(&chunk[..n]);
            let _ = ctx.stdout.flush();
        }
    }
    let status = child.wait();
    let _ = ctx.stderr.write_all(&errors.join().unwrap_or_default());
    match status.ok().and_then(|s| s.code()) {
        Some(0) => Ok(()),
        Some(code) => Err(code),
        None => Err(exit::RUNTIME_ERROR),
    }
}

fn test(target: &Target<'_>, filter: Option<&str>, ctx: &mut Context<'_>) -> CmdResult {
    let inputs = load_inputs(target, false, ctx)?;
    let several = inputs.len() > 1;
    let mut result = Ok(());
    for input in inputs {
        if several {
            let _ = writeln!(ctx.stderr, "testing {}", input.name());
        }
        result = result.and(test_one(&input, filter, ctx));
    }
    result
}

fn test_one(input: &Input, filter: Option<&str>, ctx: &mut Context<'_>) -> CmdResult {
    let (module, map, tests) = compile(input, None, true, ctx)?;
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

fn gen_ts(
    target: &Target<'_>,
    output: Option<&Path>,
    angular: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let input = load_input(target, ctx)?;
    let (checked, map) = analyze(&input, None, true, ctx)?;
    let dependencies: Vec<&str> = input
        .packages
        .iter()
        .flat_map(|g| &g.packages[1..])
        .map(brim_package::Package::name)
        .collect();
    // The program's own modules: not those of dependencies or `std`.
    let own = |m: usize| {
        let name = &checked.modules[m];
        let first = name.split('.').next().unwrap_or_default();
        name.is_empty() || (first != "std" && !dependencies.contains(&first))
    };
    let options = brim_tsgen::Options { angular };
    let text = match brim_tsgen::generate(&checked, own, input.file.name(), options) {
        Ok(text) => text,
        Err(diags) => {
            report(&diags, &map, ctx);
            return Err(exit::COMPILE_ERROR);
        }
    };
    match output {
        None => write_out(ctx.stdout, &text),
        Some(path) => {
            let full = ctx.cwd.join(path);
            let written = full
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&full, text));
            if let Err(e) = written {
                let shown = path.display().to_string();
                return Err(input_error(
                    ctx,
                    &shown,
                    Diagnostic::error(format!("cannot write file: {e}")),
                ));
            }
            let _ = writeln!(ctx.stderr, "wrote {}", path.display());
            Ok(())
        }
    }
}

/// Where builds keep what they can reuse: `$BRIM_HOME/cache`, unless
/// `BRIM_NO_CACHE` is set.
fn build_cache_dir() -> Option<PathBuf> {
    if std::env::var_os("BRIM_NO_CACHE").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    brim_package::default_home().map(|home| home.join("cache"))
}

/// The file that records what an output was last built from, and what it
/// would be built from now: the compiler (path and time, so a rebuilt
/// compiler counts), the options, the C compiler, every source file, and
/// the C code (with the other files in its directories, such as headers)
/// and libraries of `extern fn`s.
fn build_stamp(
    cache: &Path,
    output: &Path,
    map: &SourceMap,
    release: bool,
    native: &NativeInputs,
) -> (PathBuf, String) {
    let hex = |bytes: &[u8]| -> String {
        brim_runtime::crypto::sha256(bytes)[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let compiler = std::env::current_exe().ok();
    let compiler_time = compiler
        .as_ref()
        .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut key = format!(
        "{}\n{compiler:?}\n{compiler_time}\n{release}\n{:?}\n{:?}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::var("BRIM_CC").ok(),
        std::env::var("CC").ok(),
    );
    for file in map.files() {
        key.push_str(file.name());
        key.push('\n');
        key.push_str(&hex(file.text().as_bytes()));
        key.push('\n');
    }
    let mut dirs: Vec<&Path> = native.c_sources.iter().filter_map(|p| p.parent()).collect();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|e| e.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for file in files.iter().filter(|f| f.is_file()) {
            key.push_str(&format!("{}\n", file.display()));
            key.push_str(&hex(&std::fs::read(file).unwrap_or_default()));
            key.push('\n');
        }
    }
    key.push_str(&native.link.join(","));
    let path = cache
        .join("builds")
        .join(hex(output.display().to_string().as_bytes()));
    (path, hex(key.as_bytes()))
}

fn doc(target: &Target<'_>, output: Option<&Path>, std: bool, ctx: &mut Context<'_>) -> CmdResult {
    if std {
        return doc_one(None, output, false, ctx);
    }
    let inputs = load_inputs(target, false, ctx)?;
    let several = inputs.len() > 1;
    if several && output.is_some() {
        let _ = writeln!(
            ctx.stderr,
            "error: `--output` needs one package, and this workspace has several\n  \
             = help: pick one with `-p NAME`"
        );
        return Err(exit::USAGE);
    }
    let mut result = Ok(());
    for input in inputs {
        result = result.and(doc_one(Some(input), output, several, ctx));
    }
    result
}

/// Documents one program, or the standard library without `input`. With
/// `own_dir`, the default directory is `target/doc/<package>`.
fn doc_one(
    input: Option<Input>,
    output: Option<&Path>,
    own_dir: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    // The parsed modules with their texts, and the title.
    let mut parsed: Vec<(String, brim_ast::Program, String, usize)> = Vec::new();
    let (title, default_dir) = if let Some(input) = input {
        let (title, dir) = doc_program(&input, &mut parsed, ctx)?;
        let dir = if own_dir { dir.join(&title) } else { dir };
        (title, dir)
    } else {
        for (name, text) in brim_driver::STD_MODULES {
            let (program, diags) = brim_parser::parse(text);
            if brim_diagnostics::has_errors(&diags) {
                let _ = writeln!(ctx.stderr, "error: {name} does not parse");
                return Err(exit::INTERNAL);
            }
            parsed.push(((*name).to_string(), program, (*text).to_string(), 0));
        }
        ("Brim standard library".to_string(), PathBuf::from("doc"))
    };
    let modules: Vec<brim_doc::Module> = parsed
        .iter()
        .map(|(name, program, text, base)| brim_doc::Module {
            name: name.clone(),
            program,
            text,
            base: *base,
        })
        .collect();
    let files = brim_doc::generate(&modules, &title);
    let dir = output.map_or(default_dir, Path::to_path_buf);
    let full = ctx.cwd.join(&dir);
    for (name, contents) in &files {
        let written =
            std::fs::create_dir_all(&full).and_then(|()| std::fs::write(full.join(name), contents));
        if let Err(e) = written {
            let shown = dir.join(name).display().to_string();
            return Err(input_error(
                ctx,
                &shown,
                Diagnostic::error(format!("cannot write file: {e}")),
            ));
        }
    }
    let _ = writeln!(ctx.stderr, "wrote {}", dir.join("index.html").display());
    Ok(())
}

/// Collects the documented modules of a program; returns the title and the
/// default directory.
fn doc_program(
    input: &Input,
    parsed: &mut Vec<(String, brim_ast::Program, String, usize)>,
    ctx: &mut Context<'_>,
) -> Result<(String, PathBuf), i32> {
    let cwd = ctx.cwd.clone();
    let loaded = brim_driver::load(&input.file, input.packages.as_ref(), &mut |path| {
        std::fs::read(cwd.join(path))
    });
    if brim_diagnostics::has_errors(&loaded.diags) {
        report(&loaded.diags, &loaded.map, ctx);
        return Err(exit::COMPILE_ERROR);
    }
    let dependencies: Vec<&str> = input
        .packages
        .iter()
        .flat_map(|g| &g.packages[1..])
        .map(brim_package::Package::name)
        .collect();
    let package = input.packages.as_ref().map_or_else(
        || {
            Path::new(input.file.name())
                .file_stem()
                .map_or("program".to_string(), |s| s.to_string_lossy().into_owned())
        },
        |g| g.packages[0].name().to_string(),
    );
    for (i, module) in loaded.modules.iter().enumerate() {
        let first = module.name.split('.').next().unwrap_or_default();
        if first == "std" || dependencies.contains(&first) {
            continue;
        }
        // The root module by its file's name: `main`.
        let name = if module.name.is_empty() {
            Path::new(input.file.name())
                .file_stem()
                .map_or("main".to_string(), |s| s.to_string_lossy().into_owned())
        } else {
            module.name.clone()
        };
        let text = loaded.map.files()[i].text().to_string();
        // The modules' spans are offsets into the whole program.
        let base = loaded.map.base(i);
        parsed.push((name, module.program.clone(), text, base));
    }
    let dir = match &input.packages {
        Some(_) => input
            .default_output
            .parent()
            .map_or(PathBuf::from("target"), Path::to_path_buf)
            .join("doc"),
        None => input
            .default_output
            .parent()
            .map_or(PathBuf::from("doc"), |p| p.join("doc")),
    };
    Ok((package, dir))
}

fn update(target: &Target<'_>, ctx: &mut Context<'_>) -> CmdResult {
    let inputs = load_inputs(target, true, ctx)?;
    let own: Vec<&str> = inputs.iter().map(Input::name).collect();
    let mut dependencies: Vec<&str> = inputs
        .iter()
        .flat_map(|i| i.packages.iter().flat_map(|g| &g.packages[1..]))
        .map(brim_package::Package::name)
        .filter(|name| !own.contains(name))
        .collect();
    dependencies.sort_unstable();
    dependencies.dedup();
    if inputs.iter().all(|i| i.packages.is_none()) {
        let _ = writeln!(
            ctx.stderr,
            "error: `brim update` works on a project, not a single file\n  \
             = help: pass the project's directory or brim.toml"
        );
        return Err(exit::USAGE);
    }
    let count = dependencies.len();
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
    target: &Target<'_>,
    output: Option<&Path>,
    release: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let inputs = load_inputs(target, false, ctx)?;
    if inputs.len() > 1 && output.is_some() {
        let _ = writeln!(
            ctx.stderr,
            "error: `--output` needs one package, and this workspace has several\n  \
             = help: pick one with `-p NAME`"
        );
        return Err(exit::USAGE);
    }
    let several = inputs.len() > 1;
    let mut result = Ok(());
    for input in inputs {
        // At a workspace's root, libraries have nothing to build.
        if several && input.lib {
            let _ = writeln!(ctx.stderr, "skipped {} (a library)", input.name());
            continue;
        }
        result = result.and(build_one(input, output, release, ctx));
    }
    result
}

fn build_one(
    input: Input,
    output: Option<&Path>,
    release: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let (module, map, _) = compile(&input, None, false, ctx)?;
    built_executable(input, module, map, output, release, ctx)
}

/// Builds the executable of a compiled program, unless it is up to date.
fn built_executable(
    input: Input,
    module: Module,
    map: SourceMap,
    output: Option<&Path>,
    release: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let native = native_inputs(&input, &ctx.cwd);
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
    let cache_dir = build_cache_dir();
    // Nothing changed since the last build of this output: keep it.
    let stamp = cache_dir
        .as_ref()
        .map(|dir| build_stamp(dir, &output, &map, release, &native));
    if let Some((path, key)) = &stamp
        && output.is_file()
        && std::fs::read_to_string(path).is_ok_and(|k| k == *key)
    {
        let _ = writeln!(ctx.stderr, "built {} (up to date)", shown.display());
        return Ok(());
    }
    let options = brim_codegen::BuildOptions {
        output,
        opt_level: if release {
            brim_codegen::OptLevel::Release
        } else {
            brim_codegen::OptLevel::Debug
        },
        source: map,
        cache_dir,
        c_sources: native.c_sources,
        link: native.link,
    };
    match brim_codegen::native_backend().build(&module, &options) {
        Ok(_) => {
            if let Some((path, key)) = stamp
                && let Some(dir) = path.parent()
            {
                let _ = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, key));
            }
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

/// The C code and libraries of a program's packages, for native builds.
#[derive(Debug, Default)]
struct NativeInputs {
    c_sources: Vec<PathBuf>,
    link: Vec<String>,
}

fn native_inputs(input: &Input, cwd: &Path) -> NativeInputs {
    let mut native = NativeInputs::default();
    for package in input.packages.iter().flat_map(|g| &g.packages) {
        let build = &package.manifest.build;
        native.c_sources.extend(
            build
                .c_sources
                .iter()
                .map(|source| cwd.join(&package.dir).join(source)),
        );
        for library in &build.link {
            if !native.link.contains(library) {
                native.link.push(library.clone());
            }
        }
    }
    native
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
    let (checked, map) = analyze(input, emit, tests, ctx)?;
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

/// Parses, loads the imported modules and checks the program. Prints
/// diagnostics (and `--emit tokens|ast` output).
fn analyze(
    input: &Input,
    emit: Option<Emit>,
    tests: bool,
    ctx: &mut Context<'_>,
) -> Result<(brim_semantic::checked::CheckedProgram, SourceMap), i32> {
    let file = &input.file;
    if emit == Some(Emit::Tokens) {
        let (tokens, _) = brim_lexer::lex(file.text());
        for token in &tokens {
            let loc = file.location(token.span.start);
            let _ = writeln!(ctx.stdout, "{loc}\t{:?}", token.kind);
        }
    }
    let cwd = ctx.cwd.clone();
    let brim_driver::Loaded {
        modules,
        map,
        mut diags,
    } = brim_driver::load(file, input.packages.as_ref(), &mut |path| {
        std::fs::read(cwd.join(path))
    });
    // The syntax tree of the root file, if it has no syntax errors.
    let root_ok = !diags.iter().any(|d| {
        d.severity == Severity::Error && d.span().is_none_or(|s| s.start <= file.text().len())
    });
    if emit == Some(Emit::Ast) && root_ok {
        let _ = writeln!(ctx.stdout, "{:#?}", modules[0].program);
    }
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
    match checked {
        Some(checked) => Ok((checked, map)),
        None => Err(exit::COMPILE_ERROR),
    }
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
    /// A package without a program: `file` is its library root, which
    /// needs no `main`.
    lib: bool,
}

impl Input {
    /// The package's name, or the file's for a single file.
    fn name(&self) -> &str {
        match &self.packages {
            Some(graph) => graph.packages[0].name(),
            None => self.file.name(),
        }
    }
}

/// What a project command works on: the PATH argument and `-p`.
#[derive(Clone, Copy, Debug)]
struct Target<'a> {
    path: Option<&'a Path>,
    package: Option<&'a str>,
}

impl<'a> Target<'a> {
    fn new(path: &'a Option<PathBuf>, package: &'a Option<String>) -> Target<'a> {
        Target {
            path: path.as_deref(),
            package: package.as_deref(),
        }
    }
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

/// The one program a command such as `brim run` works on.
fn load_input(target: &Target<'_>, ctx: &mut Context<'_>) -> Result<Input, i32> {
    let mut inputs = load_inputs(target, false, ctx)?;
    if inputs.len() > 1 {
        let names: Vec<&str> = inputs.iter().map(Input::name).collect();
        let _ = writeln!(
            ctx.stderr,
            "error: this workspace has several packages: {}\n  \
             = help: pick one with `-p NAME`",
            names.join(", ")
        );
        return Err(exit::USAGE);
    }
    Ok(inputs.remove(0))
}

/// The manifest a command's PATH stands for, or the single source file.
enum Located {
    Manifest(PathBuf),
    File(Input),
}

fn locate(path: Option<&Path>, ctx: &mut Context<'_>) -> Result<Located, i32> {
    match path {
        None => match brim_manifest::find(&ctx.cwd) {
            Some(found) => {
                let shown = found
                    .strip_prefix(&ctx.cwd)
                    .map(Path::to_path_buf)
                    .unwrap_or(found);
                Ok(Located::Manifest(shown))
            }
            None => {
                let _ = writeln!(
                    ctx.stderr,
                    "error: no input given, and no {} found in this directory or its parents\n  \
                     = help: pass a `.brim` file, e.g. `brim run examples/hello.brim`",
                    brim_manifest::MANIFEST_FILE
                );
                Err(exit::USAGE)
            }
        },
        Some(path) => {
            let full = ctx.cwd.join(path);
            if full.is_dir() {
                Ok(Located::Manifest(path.join(brim_manifest::MANIFEST_FILE)))
            } else if path
                .file_name()
                .is_some_and(|n| n == brim_manifest::MANIFEST_FILE)
            {
                Ok(Located::Manifest(path.to_path_buf()))
            } else if path.extension().is_some_and(|e| e == "brim") {
                Ok(Located::File(Input {
                    file: read_source(path, ctx, None)?,
                    default_output: path.with_extension(std::env::consts::EXE_EXTENSION),
                    packages: None,
                    lib: false,
                }))
            } else if !full.exists() {
                let shown = path.display().to_string();
                Err(input_error(
                    ctx,
                    &shown,
                    Diagnostic::error("file not found"),
                ))
            } else {
                let shown = path.display().to_string();
                Err(input_error(
                    ctx,
                    &shown,
                    Diagnostic::error("not a Brim source file")
                        .with_help("Brim source files use the `.brim` extension")
                        .with_help("to build a project, pass its directory or brim.toml"),
                ))
            }
        }
    }
}

/// Reads and validates a package manifest (or workspace root), printing
/// its errors.
fn read_manifest(
    path: &Path,
    ctx: &mut Context<'_>,
) -> Result<(SourceFile, brim_manifest::Root), i32> {
    let file = read_text(path, ctx)?;
    match brim_manifest::parse_root(file.text()) {
        Ok(root) => Ok((file, root)),
        Err(diags) => {
            let renderer = Renderer::new(ctx.color);
            for diag in &diags {
                let _ = writeln!(ctx.stderr, "{}", renderer.render(diag, &file));
            }
            Err(exit::INPUT)
        }
    }
}

/// The programs a project command works on: one package, the `-p` member
/// of a workspace, the member whose directory the command runs in, or
/// every member at a workspace's root. `update` picks the newest
/// dependency versions.
fn load_inputs(
    target: &Target<'_>,
    update: bool,
    ctx: &mut Context<'_>,
) -> Result<Vec<Input>, i32> {
    let manifest_path = match locate(target.path, ctx)? {
        Located::File(input) => {
            if target.package.is_some() {
                let _ = writeln!(
                    ctx.stderr,
                    "error: `-p` picks a package of a workspace, not of a single file"
                );
                return Err(exit::USAGE);
            }
            return Ok(vec![input]);
        }
        Located::Manifest(path) => path,
    };
    let (manifest_file, root) = read_manifest(&manifest_path, ctx)?;
    let project_dir = manifest_path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let workspace = match &root.workspace {
        Some(ws) => Some((project_dir.clone(), manifest_file.clone(), ws.clone(), true)),
        None => brim_package::find_root(&ctx.cwd, &project_dir).and_then(|(dir, file)| {
            let ws = brim_manifest::parse_root(file.text()).ok()?.workspace?;
            Some((dir, file, ws, false))
        }),
    };
    let Some((ws_dir, ws_file, ws, at_root)) = workspace else {
        let manifest = root
            .package
            .expect("a manifest without [workspace] has a package");
        if let Some(name) = target.package
            && name != manifest.package.name
        {
            let _ = writeln!(
                ctx.stderr,
                "error: there is no package `{name}` here; this is the package `{}`",
                manifest.package.name
            );
            return Err(exit::USAGE);
        }
        let options = package_options(update, ctx)?;
        let lock = project_dir.join(brim_package::LOCK_FILE);
        let graph = match brim_package::resolve(&project_dir, manifest, manifest_file, &options) {
            Ok((graph, report)) => {
                print_report(&report, &lock, ctx);
                graph
            }
            Err(failure) => return Err(report_failure(failure, ctx)),
        };
        let target_dir = project_dir.join("target");
        return Ok(vec![package_input(
            &project_dir,
            &manifest_path,
            &target_dir,
            graph,
            ctx,
        )?]);
    };

    // A workspace: the root package (if any) and the members.
    let mut members: Vec<brim_package::Member> = Vec::new();
    let root_package = if at_root {
        root.package.clone()
    } else {
        brim_manifest::parse_root(ws_file.text())
            .ok()
            .and_then(|r| r.package)
    };
    if let Some(manifest) = root_package {
        members.push(brim_package::Member {
            dir: ws_dir.clone(),
            manifest,
            manifest_file: ws_file.clone(),
            span: None,
        });
    }
    let dirs = match brim_package::member_dirs(&ctx.cwd, &ws_dir, &ws, &ws_file) {
        Ok(dirs) => dirs,
        Err(failure) => return Err(report_failure(failure, ctx)),
    };
    for (dir, span) in dirs {
        if members.iter().any(|m| same_dir(&ctx.cwd, &m.dir, &dir)) {
            continue;
        }
        let (manifest_file, member_root) =
            read_manifest(&dir.join(brim_manifest::MANIFEST_FILE), ctx)?;
        let Some(manifest) = member_root.package else {
            let _ = writeln!(
                ctx.stderr,
                "error: the workspace member {} has no `[package]`",
                dir.display()
            );
            return Err(exit::INPUT);
        };
        members.push(brim_package::Member {
            dir,
            manifest,
            manifest_file,
            span: Some(span),
        });
    }
    let options = package_options(update, ctx)?;
    let graphs = match brim_package::resolve_workspace(&ws_dir, ws_file, &members, &options) {
        Ok((graphs, report)) => {
            print_report(&report, &ws_dir.join(brim_package::LOCK_FILE), ctx);
            graphs
        }
        Err(failure) => return Err(report_failure(failure, ctx)),
    };
    let names = || -> String {
        members
            .iter()
            .map(|m| m.manifest.package.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let selected: Vec<usize> = match target.package {
        Some(name) => match members.iter().position(|m| m.manifest.package.name == name) {
            Some(i) => vec![i],
            None => {
                let _ = writeln!(
                    ctx.stderr,
                    "error: the workspace has no package `{name}`\n  = note: its packages: {}",
                    names()
                );
                return Err(exit::USAGE);
            }
        },
        None if !at_root => members
            .iter()
            .position(|m| same_dir(&ctx.cwd, &m.dir, &project_dir))
            .into_iter()
            .collect(),
        None => (0..members.len()).collect(),
    };
    if selected.is_empty() {
        let _ = writeln!(ctx.stderr, "error: the workspace has no packages");
        return Err(exit::INPUT);
    }
    let target_dir = ws_dir.join("target");
    let mut inputs = Vec::new();
    for i in selected {
        let member = &members[i];
        let manifest_path = member.dir.join(brim_manifest::MANIFEST_FILE);
        inputs.push(package_input(
            &member.dir,
            &manifest_path,
            &target_dir,
            graphs[i].clone(),
            ctx,
        )?);
    }
    Ok(inputs)
}

/// Whether two directories relative to `cwd` are the same.
fn same_dir(cwd: &Path, a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| cwd.join(p).canonicalize().ok();
    a == b || (canonical(a).is_some() && canonical(a) == canonical(b))
}

/// The program of the package in `dir`, whose dependencies are `graph`.
fn package_input(
    dir: &Path,
    manifest_path: &Path,
    target_dir: &Path,
    graph: brim_package::Graph,
    ctx: &mut Context<'_>,
) -> Result<Input, i32> {
    let manifest = &graph.packages[0].manifest;
    let mut entry = shown_join(dir, &manifest.build.entry);
    // A library without a program: check and test its library root.
    let lib = shown_join(dir, &manifest.build.lib);
    let is_lib = !ctx.cwd.join(&entry).exists() && ctx.cwd.join(&lib).is_file();
    if is_lib {
        entry = lib;
    }
    let note = format!(
        "the entry file is set by `[build] entry` in {} (default `{}`)",
        manifest_path.display(),
        brim_manifest::DEFAULT_ENTRY
    );
    let default_output = target_dir
        .join(&manifest.package.name)
        .with_extension(std::env::consts::EXE_EXTENSION);
    Ok(Input {
        file: read_source(&entry, ctx, Some(note))?,
        default_output,
        packages: Some(graph),
        lib: is_lib,
    })
}

/// `dir/path` as paths are shown: with `/` on every platform.
fn shown_join(dir: &Path, path: &Path) -> PathBuf {
    let slash = |p: &Path| p.display().to_string().replace('\\', "/");
    let dir = slash(dir);
    if dir.is_empty() || dir == "." {
        return PathBuf::from(slash(path));
    }
    PathBuf::from(format!("{}/{}", dir.trim_end_matches('/'), slash(path)))
}

/// How to resolve dependencies here.
fn package_options(update: bool, ctx: &mut Context<'_>) -> Result<brim_package::Options, i32> {
    let Some(home) = brim_package::default_home() else {
        let _ = writeln!(
            ctx.stderr,
            "error: cannot find a home directory for the package cache\n  \
             = help: set BRIM_HOME to a directory brim can use"
        );
        return Err(exit::INPUT);
    };
    Ok(brim_package::Options {
        cwd: ctx.cwd.clone(),
        home,
        update,
        registry: None,
    })
}

fn print_report(report: &brim_package::Report, lock: &Path, ctx: &mut Context<'_>) {
    for what in &report.fetched {
        let _ = writeln!(ctx.stderr, "fetched {what}");
    }
    if report.lock_written {
        let _ = writeln!(ctx.stderr, "updated {}", lock.display());
    }
}

fn report_failure(failure: brim_package::Failure, ctx: &mut Context<'_>) -> i32 {
    let file = failure
        .file
        .unwrap_or_else(|| SourceFile::new(brim_manifest::MANIFEST_FILE, ""));
    let rendered = Renderer::new(ctx.color).render(&failure.diagnostic, &file);
    let _ = write!(ctx.stderr, "{rendered}");
    exit::INPUT
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
