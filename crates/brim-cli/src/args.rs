//! Command-line argument parsing.
//!
//! Parsing is separate from execution: [`parse`] turns `argv` into a
//! [`Command`] value, so adding a command means adding a variant, a help text
//! and a match arm, without touching the compiler pipeline.

use std::fmt;
use std::path::PathBuf;

/// Intermediate output that `brim check --emit` can print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emit {
    Tokens,
    Ast,
    Ir,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `brim help [command]`, `brim --help`, `brim <command> --help`.
    Help(Option<String>),
    Version,
    Check {
        input: Option<PathBuf>,
        emit: Option<Emit>,
    },
    Run {
        input: Option<PathBuf>,
    },
    Build {
        input: Option<PathBuf>,
    },
}

/// A problem with the command line itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError {
    pub message: String,
    /// The command whose usage should be suggested, if any.
    pub command: Option<&'static str>,
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

const COMMANDS: &[&str] = &["check", "run", "build", "help", "version"];

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, UsageError> {
    let Some((first, rest)) = args.split_first() else {
        return Err(UsageError {
            message: "no command given".into(),
            command: None,
        });
    };
    match first.as_str() {
        "-h" | "--help" => Ok(Command::Help(None)),
        "-V" | "--version" | "version" => {
            if let Some(extra) = rest.first() {
                return Err(unexpected(extra, None));
            }
            Ok(Command::Version)
        }
        "help" => match rest {
            [] => Ok(Command::Help(None)),
            [cmd] if COMMANDS.contains(&cmd.as_str()) => Ok(Command::Help(Some(cmd.clone()))),
            [cmd] => Err(unknown_command(cmd)),
            [_, extra, ..] => Err(unexpected(extra, None)),
        },
        "check" => {
            let parsed = CommandArgs::parse("check", rest, true)?;
            Ok(parsed.help.unwrap_or(Command::Check {
                input: parsed.input,
                emit: parsed.emit,
            }))
        }
        "run" => {
            let parsed = CommandArgs::parse("run", rest, false)?;
            Ok(parsed.help.unwrap_or(Command::Run {
                input: parsed.input,
            }))
        }
        "build" => {
            let parsed = CommandArgs::parse("build", rest, false)?;
            Ok(parsed.help.unwrap_or(Command::Build {
                input: parsed.input,
            }))
        }
        other if other.starts_with('-') => Err(UsageError {
            message: format!("unknown option `{other}`"),
            command: None,
        }),
        other => Err(unknown_command(other)),
    }
}

fn unknown_command(cmd: &str) -> UsageError {
    UsageError {
        message: format!(
            "unknown command `{cmd}` (available: {})",
            COMMANDS.join(", ")
        ),
        command: None,
    }
}

fn unexpected(arg: &str, command: Option<&'static str>) -> UsageError {
    UsageError {
        message: format!("unexpected argument `{arg}`"),
        command,
    }
}

/// Arguments shared by `check`, `run` and `build`.
struct CommandArgs {
    input: Option<PathBuf>,
    emit: Option<Emit>,
    help: Option<Command>,
}

impl CommandArgs {
    fn parse(command: &'static str, args: &[String], allow_emit: bool) -> Result<Self, UsageError> {
        let mut parsed = CommandArgs {
            input: None,
            emit: None,
            help: None,
        };
        let mut only_positional = false;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if !only_positional && arg.starts_with('-') && arg != "-" {
                match arg.as_str() {
                    "--" => only_positional = true,
                    "-h" | "--help" => parsed.help = Some(Command::Help(Some(command.into()))),
                    "--emit" if allow_emit => {
                        let value = iter.next().ok_or_else(|| UsageError {
                            message: "`--emit` needs a value: tokens, ast or ir".into(),
                            command: Some(command),
                        })?;
                        parsed.emit = Some(parse_emit(value, command)?);
                    }
                    a if allow_emit && a.starts_with("--emit=") => {
                        parsed.emit = Some(parse_emit(&a["--emit=".len()..], command)?);
                    }
                    other => {
                        return Err(UsageError {
                            message: format!("unknown option `{other}` for `brim {command}`"),
                            command: Some(command),
                        });
                    }
                }
            } else if parsed.input.is_none() {
                parsed.input = Some(PathBuf::from(arg));
            } else {
                return Err(unexpected(arg, Some(command)));
            }
        }
        Ok(parsed)
    }
}

fn parse_emit(value: &str, command: &'static str) -> Result<Emit, UsageError> {
    match value {
        "tokens" => Ok(Emit::Tokens),
        "ast" => Ok(Emit::Ast),
        "ir" => Ok(Emit::Ir),
        other => Err(UsageError {
            message: format!("invalid `--emit` value `{other}` (expected tokens, ast or ir)"),
            command: Some(command),
        }),
    }
}

pub const MAIN_HELP: &str = "\
Brim: the Ordnary programming language

Usage: brim <COMMAND> [OPTIONS]

Commands:
  check [PATH]   Check a program for errors without running it
  run [PATH]     Check a program and run it with the reference interpreter
  build [PATH]   Compile a native executable (not available yet)
  help [COMMAND] Show help for a command
  version        Show the compiler version

PATH is a `.brim` file, a `brim.toml` manifest, or a directory containing
one. Without PATH, the nearest `brim.toml` in the current directory or its
parents is used.

Options:
  -h, --help     Show help
  -V, --version  Show the compiler version
";

pub fn command_help(command: &str) -> &'static str {
    match command {
        "check" => {
            "\
Check a program for errors without running it

Usage: brim check [OPTIONS] [PATH]

Options:
  --emit <tokens|ast|ir>  Print an intermediate representation to stdout
  -h, --help              Show help
"
        }
        "run" => {
            "\
Check a program and run it with the reference interpreter

The interpreter is a temporary execution path while the native backend is
developed. Program output goes to stdout; diagnostics go to stderr.

Usage: brim run [PATH]

Options:
  -h, --help  Show help
"
        }
        "build" => {
            "\
Compile a native executable

Not available yet: the LLVM backend has not been implemented. The program is
still checked and lowered to IR, and the command reports the missing backend.
Use `brim run` to execute programs in the meantime.

Usage: brim build [PATH]

Options:
  -h, --help  Show help
"
        }
        "help" => "Show help for a command\n\nUsage: brim help [COMMAND]\n",
        "version" => "Show the compiler version\n\nUsage: brim version\n",
        _ => MAIN_HELP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, UsageError> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn global_flags() {
        assert_eq!(p(&["--help"]), Ok(Command::Help(None)));
        assert_eq!(p(&["-V"]), Ok(Command::Version));
        assert_eq!(p(&["version"]), Ok(Command::Version));
        assert_eq!(p(&["help", "run"]), Ok(Command::Help(Some("run".into()))));
        assert!(p(&[]).is_err());
    }

    #[test]
    fn commands() {
        assert_eq!(
            p(&["check", "a.brim"]),
            Ok(Command::Check {
                input: Some("a.brim".into()),
                emit: None
            })
        );
        assert_eq!(
            p(&["check", "--emit", "ir", "a.brim"]),
            Ok(Command::Check {
                input: Some("a.brim".into()),
                emit: Some(Emit::Ir)
            })
        );
        assert_eq!(
            p(&["check", "--emit=ast"]),
            Ok(Command::Check {
                input: None,
                emit: Some(Emit::Ast)
            })
        );
        assert_eq!(p(&["run"]), Ok(Command::Run { input: None }));
        assert_eq!(
            p(&["run", "--", "-weird.brim"]),
            Ok(Command::Run {
                input: Some("-weird.brim".into())
            })
        );
        assert_eq!(p(&["run", "-h"]), Ok(Command::Help(Some("run".into()))));
    }

    #[test]
    fn usage_errors() {
        let msg = |args: &[&str]| p(args).unwrap_err().message;
        assert_eq!(
            msg(&["frobnicate"]),
            "unknown command `frobnicate` (available: check, run, build, help, version)"
        );
        assert_eq!(
            msg(&["run", "a.brim", "b.brim"]),
            "unexpected argument `b.brim`"
        );
        assert_eq!(
            msg(&["run", "--emit", "ir"]),
            "unknown option `--emit` for `brim run`"
        );
        assert_eq!(
            msg(&["check", "--emit", "llvm"]),
            "invalid `--emit` value `llvm` (expected tokens, ast or ir)"
        );
        assert_eq!(
            msg(&["check", "--emit"]),
            "`--emit` needs a value: tokens, ast or ir"
        );
        assert_eq!(msg(&["--frob"]), "unknown option `--frob`");
    }
}
