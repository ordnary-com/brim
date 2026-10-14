//! The `brim` command-line tool.
//!
//! The library half of the crate contains everything except process setup,
//! so commands can be tested in-process: [`run`] takes the arguments and
//! explicit output streams and returns an exit code (see [`exit`]).

pub mod args;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use args::{Command, Emit};
use brim_diagnostics::{ColorChoice, Diagnostic, Renderer, Severity, SourceFile, Span};
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
    /// Diagnostics and status messages.
    pub stderr: &'a mut dyn Write,
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
        Command::Run { input } => run_program(input.as_deref(), ctx),
        Command::Build {
            input,
            output,
            release,
        } => build(input.as_deref(), output.as_deref(), release, ctx),
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
    let file = load_input(input, ctx)?.file;
    compile(&file, emit, ctx)?;
    let _ = writeln!(ctx.stderr, "ok: no errors found in {}", file.name());
    Ok(())
}

fn run_program(input: Option<&Path>, ctx: &mut Context<'_>) -> CmdResult {
    let file = load_input(input, ctx)?.file;
    let module = compile(&file, None, ctx)?;
    match brim_interp::run(&module, ctx.stdout, brim_interp::Limits::default()) {
        Ok(()) => Ok(()),
        Err(trap) => {
            let mut diag = Diagnostic::error(trap.error.to_string()).with_code(trap.error.code());
            if let Some(span) = trap.span {
                diag = diag.with_primary(span, "");
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
            report(&[diag], &file, ctx);
            Err(exit::RUNTIME_ERROR)
        }
    }
}

fn build(
    input: Option<&Path>,
    output: Option<&Path>,
    release: bool,
    ctx: &mut Context<'_>,
) -> CmdResult {
    let Input {
        file,
        default_output,
    } = load_input(input, ctx)?;
    let module = compile(&file, None, ctx)?;
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
        source: file.clone(),
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
fn compile(file: &SourceFile, emit: Option<Emit>, ctx: &mut Context<'_>) -> Result<Module, i32> {
    let (tokens, mut diags) = brim_lexer::lex(file.text());
    if emit == Some(Emit::Tokens) {
        for token in &tokens {
            let loc = file.location(token.span.start);
            let _ = writeln!(ctx.stdout, "{loc}\t{:?}", token.kind);
        }
    }
    let (ast, parse_diags) = brim_parser::parse_tokens(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
    if brim_diagnostics::has_errors(&diags) {
        report(&diags, file, ctx);
        return Err(exit::COMPILE_ERROR);
    }
    if emit == Some(Emit::Ast) {
        let _ = writeln!(ctx.stdout, "{ast:#?}");
    }

    let (checked, semantic_diags) = brim_semantic::check(&ast);
    diags.extend(semantic_diags);
    report(&diags, file, ctx);
    let Some(checked) = checked else {
        return Err(exit::COMPILE_ERROR);
    };

    let module = brim_lower::lower(&checked);
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
    Ok(module)
}

/// Renders diagnostics followed by a one-line summary.
fn report(diags: &[Diagnostic], file: &SourceFile, ctx: &mut Context<'_>) {
    if diags.is_empty() {
        return;
    }
    let renderer = Renderer::new(ctx.color);
    let rendered = renderer.render_all(diags, file);
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
    let entry = manifest_path
        .parent()
        .unwrap_or(Path::new(""))
        .join(&manifest.build.entry);
    let note = format!(
        "the entry file is set by `[build] entry` in {} (default `{}`)",
        manifest_path.display(),
        brim_manifest::DEFAULT_ENTRY
    );
    let project_dir = manifest_path.parent().unwrap_or(Path::new(""));
    Ok(Input {
        file: read_source(&entry, ctx, Some(note))?,
        default_output: project_dir
            .join("target")
            .join(&manifest.package.name)
            .with_extension(std::env::consts::EXE_EXTENSION),
    })
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
