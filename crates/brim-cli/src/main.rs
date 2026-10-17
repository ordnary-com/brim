use std::io::{self, BufWriter, IsTerminal};
use std::process::ExitCode;

use brim_cli::{Context, exit};
use brim_diagnostics::ColorChoice;

/// Stack size for the compiler thread. Parsing limits expression nesting, but
/// a generous stack keeps deep-but-legal programs safe in every phase.
const STACK_SIZE: usize = 64 * 1024 * 1024;

fn main() -> ExitCode {
    let args: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect();
    let Ok(args) = args else {
        eprintln!("error: arguments must be valid UTF-8");
        return ExitCode::from(exit::USAGE as u8);
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("error: cannot determine the current directory: {e}");
            return ExitCode::from(exit::INPUT as u8);
        }
    };
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let color = if io::stderr().is_terminal() && !no_color {
        ColorChoice::Always
    } else {
        ColorChoice::Never
    };

    let worker = std::thread::Builder::new()
        .name("brim".into())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            // Not locked: tasks of an interpreted program write and read
            // from other threads.
            let mut stdout = BufWriter::new(io::stdout());
            let mut stderr = io::stderr();
            let mut stdin = io::BufReader::new(io::stdin());
            let mut ctx = Context {
                cwd,
                stdout: &mut stdout,
                stderr: &mut stderr,
                stdin: &mut stdin,
                color,
            };
            brim_cli::run(&args, &mut ctx)
        });
    let code = match worker.map(|handle| handle.join()) {
        Ok(Ok(code)) => code,
        Ok(Err(_panic)) => {
            eprintln!(
                "error: internal compiler error: the compiler panicked\n  \
                 = note: this is a bug in the Brim compiler; please report it"
            );
            exit::INTERNAL
        }
        Err(e) => {
            eprintln!("error: cannot start the compiler thread: {e}");
            exit::INTERNAL
        }
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
