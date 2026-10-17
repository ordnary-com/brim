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
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        emit: Option<Emit>,
    },
    Run {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        /// Arguments for the program (`args()`), after its path.
        args: Vec<String>,
    },
    Build {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        /// `-o`/`--output`: where to write the executable.
        output: Option<PathBuf>,
        /// `--release`: optimise.
        release: bool,
    },
    Test {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        /// Only run tests whose names contain this text.
        filter: Option<String>,
    },
    New {
        /// The directory to create; its name is the package name.
        path: PathBuf,
    },
    /// `brim gen ts [PATH]`: TypeScript types for the program's API.
    GenTs {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        /// `-o`/`--output`: the file to write instead of standard output.
        output: Option<PathBuf>,
        /// `--angular`: also an Angular service for the API functions.
        angular: bool,
    },
    /// `brim doc [PATH]`: HTML documentation from the doc comments.
    Doc {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
        /// `-o`/`--output`: the directory to write.
        output: Option<PathBuf>,
        /// `--std`: the standard library instead of a program.
        std: bool,
    },
    /// `brim lsp`: the language server, on standard input and output.
    Lsp,
    /// `brim update [PATH]`: newest commits for git dependencies.
    Update {
        input: Option<PathBuf>,
        /// `-p`/`--package`: a package of the workspace.
        package: Option<String>,
    },
    /// `brim publish [PATH]`: upload the package to the registry.
    Publish {
        input: Option<PathBuf>,
        package: Option<String>,
        /// `--dry-run`: check and pack, but do not upload.
        dry_run: bool,
    },
    /// `brim login`: store a registry token.
    Login {
        /// `--token`; else read from standard input.
        token: Option<String>,
    },
    /// `brim yank NAME@VERSION [--undo]`.
    Yank {
        name: String,
        version: String,
        undo: bool,
    },
    Fmt {
        /// Files and directories; empty for the current project.
        paths: Vec<PathBuf>,
        /// `--check`: report unformatted files instead of changing them.
        check: bool,
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

const COMMANDS: &[&str] = &[
    "check", "run", "build", "test", "new", "fmt", "update", "publish", "login", "yank", "gen",
    "doc", "lsp", "help", "version",
];

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
            let parsed = CommandArgs::parse("check", rest)?;
            Ok(parsed.help.unwrap_or(Command::Check {
                input: parsed.input,
                package: parsed.package,
                emit: parsed.emit,
            }))
        }
        "run" => {
            let parsed = CommandArgs::parse("run", rest)?;
            Ok(parsed.help.unwrap_or(Command::Run {
                input: parsed.input,
                package: parsed.package,
                args: parsed.program_args,
            }))
        }
        "build" => {
            let parsed = CommandArgs::parse("build", rest)?;
            Ok(parsed.help.unwrap_or(Command::Build {
                input: parsed.input,
                package: parsed.package,
                output: parsed.output,
                release: parsed.release,
            }))
        }
        "test" => {
            let parsed = CommandArgs::parse("test", rest)?;
            Ok(parsed.help.unwrap_or(Command::Test {
                input: parsed.input,
                package: parsed.package,
                filter: parsed.program_args.into_iter().next(),
            }))
        }
        "new" => {
            let parsed = CommandArgs::parse("new", rest)?;
            if let Some(help) = parsed.help {
                return Ok(help);
            }
            match parsed.input {
                Some(path) => Ok(Command::New { path }),
                None => Err(UsageError {
                    message: "`brim new` needs the path of the project to create".into(),
                    command: Some("new"),
                }),
            }
        }
        "gen" => match rest.split_first() {
            None => Err(UsageError {
                message: "`brim gen` needs a target: ts".into(),
                command: Some("gen"),
            }),
            Some((flag, _)) if flag == "-h" || flag == "--help" => {
                Ok(Command::Help(Some("gen".into())))
            }
            Some((target, rest)) if target == "ts" => {
                let parsed = CommandArgs::parse("gen", rest)?;
                Ok(parsed.help.unwrap_or(Command::GenTs {
                    input: parsed.input,
                    package: parsed.package,
                    output: parsed.output,
                    angular: parsed.angular,
                }))
            }
            Some((target, _)) => Err(UsageError {
                message: format!("unknown target `{target}` for `brim gen` (available: ts)"),
                command: Some("gen"),
            }),
        },
        "doc" => {
            let parsed = CommandArgs::parse("doc", rest)?;
            Ok(parsed.help.unwrap_or(Command::Doc {
                input: parsed.input,
                package: parsed.package,
                output: parsed.output,
                std: parsed.std,
            }))
        }
        "lsp" => match rest.first().map(String::as_str) {
            None => Ok(Command::Lsp),
            Some("-h" | "--help") => Ok(Command::Help(Some("lsp".into()))),
            // Editors often pass `--stdio`; it is the only transport.
            Some("--stdio") if rest.len() == 1 => Ok(Command::Lsp),
            Some(other) => Err(unexpected(other, Some("lsp"))),
        },
        "update" => {
            let parsed = CommandArgs::parse("update", rest)?;
            Ok(parsed.help.unwrap_or(Command::Update {
                input: parsed.input,
                package: parsed.package,
            }))
        }
        "publish" => {
            let parsed = CommandArgs::parse("publish", rest)?;
            Ok(parsed.help.unwrap_or(Command::Publish {
                input: parsed.input,
                package: parsed.package,
                dry_run: parsed.dry_run,
            }))
        }
        "login" => {
            let mut token = None;
            let mut iter = rest.iter();
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-h" | "--help" => return Ok(Command::Help(Some("login".into()))),
                    "--token" => {
                        token = Some(iter.next().cloned().ok_or_else(|| UsageError {
                            message: "`--token` needs a value".into(),
                            command: Some("login"),
                        })?);
                    }
                    a if a.starts_with("--token=") => token = Some(a["--token=".len()..].into()),
                    other => return Err(unexpected(other, Some("login"))),
                }
            }
            Ok(Command::Login { token })
        }
        "yank" => {
            let mut spec = None;
            let mut undo = false;
            for arg in rest {
                match arg.as_str() {
                    "-h" | "--help" => return Ok(Command::Help(Some("yank".into()))),
                    "--undo" => undo = true,
                    a if a.starts_with('-') => {
                        return Err(UsageError {
                            message: format!("unknown option `{a}` for `brim yank`"),
                            command: Some("yank"),
                        });
                    }
                    _ if spec.is_none() => spec = Some(arg.clone()),
                    other => return Err(unexpected(other, Some("yank"))),
                }
            }
            match spec.as_deref().and_then(|s| s.split_once('@')) {
                Some((name, version)) if !name.is_empty() && !version.is_empty() => {
                    Ok(Command::Yank {
                        name: name.into(),
                        version: version.into(),
                        undo,
                    })
                }
                _ => Err(UsageError {
                    message: "`brim yank` needs NAME@VERSION, e.g. `brim yank router@1.2.0`".into(),
                    command: Some("yank"),
                }),
            }
        }
        "fmt" => {
            let mut paths = Vec::new();
            let mut check = false;
            let mut only_positional = false;
            for arg in rest {
                match arg.as_str() {
                    "--" if !only_positional => only_positional = true,
                    "-h" | "--help" if !only_positional => {
                        return Ok(Command::Help(Some("fmt".into())));
                    }
                    "--check" if !only_positional => check = true,
                    a if !only_positional && a.starts_with('-') && a != "-" => {
                        return Err(UsageError {
                            message: format!("unknown option `{a}` for `brim fmt`"),
                            command: Some("fmt"),
                        });
                    }
                    _ => paths.push(PathBuf::from(arg)),
                }
            }
            Ok(Command::Fmt { paths, check })
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

/// Arguments shared by `check`, `run` and `build`. Options are only accepted
/// by the commands they belong to: `--emit` by `check`, `--output` and
/// `--release` by `build`.
struct CommandArgs {
    input: Option<PathBuf>,
    emit: Option<Emit>,
    output: Option<PathBuf>,
    release: bool,
    /// `gen ts` only: `--angular`.
    angular: bool,
    /// `doc` only: `--std`.
    std: bool,
    /// `run` only: positional arguments after the path.
    program_args: Vec<String>,
    /// `-p`/`--package`, for project commands.
    package: Option<String>,
    /// `publish` only: `--dry-run`.
    dry_run: bool,
    help: Option<Command>,
}

impl CommandArgs {
    fn parse(command: &'static str, args: &[String]) -> Result<Self, UsageError> {
        let allow_emit = command == "check";
        let is_build = command == "build";
        let has_output = is_build || command == "gen" || command == "doc";
        let mut parsed = CommandArgs {
            input: None,
            emit: None,
            output: None,
            release: false,
            angular: false,
            std: false,
            program_args: Vec::new(),
            package: None,
            dry_run: false,
            help: None,
        };
        let takes_package = command != "new";
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
                    "-o" | "--output" if has_output => {
                        let value = iter.next().ok_or_else(|| UsageError {
                            message: if is_build {
                                format!("`{arg}` needs a value: the executable's path")
                            } else if command == "doc" {
                                format!("`{arg}` needs a value: the directory to write")
                            } else {
                                format!("`{arg}` needs a value: the file to write")
                            },
                            command: Some(command),
                        })?;
                        parsed.output = Some(PathBuf::from(value));
                    }
                    a if has_output && a.starts_with("--output=") => {
                        parsed.output = Some(PathBuf::from(&a["--output=".len()..]));
                    }
                    "-p" | "--package" if takes_package => {
                        let value = iter.next().ok_or_else(|| UsageError {
                            message: format!("`{arg}` needs a value: a package of the workspace"),
                            command: Some(command),
                        })?;
                        parsed.package = Some(value.clone());
                    }
                    a if takes_package && a.starts_with("--package=") => {
                        parsed.package = Some(a["--package=".len()..].to_string());
                    }
                    "--dry-run" if command == "publish" => parsed.dry_run = true,
                    "--release" if is_build => parsed.release = true,
                    "--angular" if command == "gen" => parsed.angular = true,
                    "--std" if command == "doc" => parsed.std = true,
                    other => {
                        return Err(UsageError {
                            message: format!("unknown option `{other}` for `brim {command}`"),
                            command: Some(command),
                        });
                    }
                }
            } else if parsed.input.is_none() {
                parsed.input = Some(PathBuf::from(arg));
            } else if command == "test" && parsed.program_args.is_empty() {
                // `brim test [PATH] [FILTER]`
                parsed.program_args.push(arg.clone());
            } else if command == "run" {
                // Everything after the program belongs to the program.
                parsed.program_args.push(arg.clone());
                only_positional = true;
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
  build [PATH]   Compile a native executable
  test [PATH]    Run a program's tests
  new <PATH>     Create a new project
  fmt [PATH...]  Format source files
  update [PATH]  Pick the newest dependency versions for brim.lock
  publish [PATH] Publish the package to the registry
  login          Store a token for the registry
  yank NAME@VER  Stop new projects from picking a published version
  gen ts [PATH]  Generate TypeScript types for the program's API
  doc [PATH]     Write HTML documentation from the doc comments
  lsp            Run the language server for editors
  help [COMMAND] Show help for a command
  version        Show the compiler version

PATH is a `.brim` file, a `brim.toml` manifest, or a directory containing
one. Without PATH, the nearest `brim.toml` in the current directory or its
parents is used. In a workspace, `-p NAME` picks a member; at the root,
check, build, test and doc work on every member.

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
  -p, --package <NAME>    A package of the workspace
  -h, --help              Show help
"
        }
        "run" => {
            "\
Check a program and run it with the reference interpreter

The interpreter is a temporary execution path while the native backend is
developed. Program output goes to stdout; diagnostics go to stderr.

Usage: brim run [PATH] [ARGS...]

ARGS are passed to the program (`args()`). The program's exit code (see
`exit`) becomes the exit code of `brim run`.

Options:
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "build" => {
            "\
Compile a native executable

The program is compiled with LLVM and linked with the system C compiler
(`$BRIM_CC`, `$CC` or `cc`). The executable is written next to a `.brim`
file, without its extension, or to `target/<package name>` for a project.

Native builds need a `brim` compiled with the `llvm` feature. Without it,
the program is still checked and the command reports the missing backend.

Usage: brim build [OPTIONS] [PATH]

Options:
  -o, --output <PATH>   Where to write the executable
  --release             Optimise the executable
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "test" => {
            "\
Run a program's tests

A test is a function whose name starts with `test_`, outside `impl`
blocks, in any module of the program. It takes no parameters and returns
nothing; it fails when it stops with a run-time error, such as a failed
`assert` or `assert_eq`. `main` is not needed, and not run.

Tests run with the reference interpreter, one after another. Their output
is shown only when they fail.

Usage: brim test [PATH] [FILTER]

FILTER runs only the tests whose names contain it.

Options:
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "new" => {
            "\
Create a new project

Creates the directory PATH with a `brim.toml` manifest, `src/main.brim`
and a `.gitignore`. The directory's name is the package name.

Usage: brim new <PATH>

Options:
  -h, --help  Show help
"
        }
        "doc" => {
            "\
Write HTML documentation from the doc comments

Usage: brim doc [OPTIONS] [PATH]

Documents the `pub` items of the program's own modules: the `//` comment
lines right above each item, and the comment at the top of each file. In a
comment, `code` is code and lines indented by two or more spaces are code
examples. Writes `target/doc` in a project, `doc` next to a single file.

Options:
  -o, --output <DIR>    Write to DIR
      --std             Document the standard library instead
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "gen" => {
            "\
Generate TypeScript types for the program's API

Writes a TypeScript declaration for every `pub` struct and enum of the
program's own modules (not those of dependencies), and for the types they
use. The declarations describe the values as JSON, for a web frontend that
talks to a Brim backend. See the language specification for the mapping.

`main` is not needed: a package with only API types works.

Usage: brim gen ts [OPTIONS] [PATH]

Writes a TypeScript declaration for every `pub` struct and enum. With
--angular it also writes an Angular service for each module served with
`serve_api`, with a method per API function.

Options:
  -o, --output <FILE>   Write to FILE instead of standard output
      --angular         Also write an Angular client for the API functions
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "lsp" => {
            "\
Run the language server for editors

Speaks the Language Server Protocol on standard input and output:
diagnostics, formatting, document symbols, go to definition and hover.
Editors start it themselves; see editors/ in the Brim repository.

Usage: brim lsp [--stdio]

Options:
  -h, --help  Show help
"
        }
        "update" => {
            "\
Pick the newest dependency versions for brim.lock

Fetches every git dependency of the project and records the newest commit
of its branch, tag or default branch in `brim.lock`, and picks the newest
version of every registry dependency that meets its requirements. Other
commands use what `brim.lock` says and fetch only what is missing.

Usage: brim update [OPTIONS] [PATH]

Options:
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "publish" => {
            "\
Publish the package to the registry

Checks the package, packs `brim.toml`, `src/` and the README, LICENSE and
CHANGELOG files at the top into an archive, and uploads it with the token
from `brim login`. Every dependency needs a `version`; a `path` dependency
may have one, which the published manifest keeps. A published version can
never change: publish a new version instead.

The registry is `$BRIM_REGISTRY`, or the default registry.

Usage: brim publish [OPTIONS] [PATH]

Options:
  --dry-run             Check and pack, but do not upload
  -p, --package <NAME>  A package of the workspace
  -h, --help            Show help
"
        }
        "login" => {
            "\
Store a token for the registry

Reads an API token from standard input (or `--token`), checks it with the
registry and stores it in `$BRIM_HOME/credentials.toml`. The environment
variable BRIM_REGISTRY_TOKEN, when set, is used instead.

Usage: brim login [--token <TOKEN>]

Options:
  -h, --help  Show help
"
        }
        "yank" => {
            "\
Stop new projects from picking a published version

A yanked version stays available to projects whose `brim.lock` names it,
but is not picked for new lock files. `--undo` takes the yank back.

Usage: brim yank NAME@VERSION [--undo]

Options:
  -h, --help  Show help
"
        }
        "fmt" => {
            "\
Format source files

Formats each `.brim` file in place. A directory stands for the `.brim`
files in it and its subdirectories (except `target` and hidden ones).
Without PATH, the current project is formatted: the directory of the
nearest `brim.toml`. `-` formats standard input to standard output.

Formatting keeps comments, blank lines (at most one in a row) and line
breaks, and sets indentation and the spaces between tokens. Files with
syntax errors are reported and left unchanged.

Usage: brim fmt [OPTIONS] [PATH...]

Options:
  --check     Change nothing; list the files that are not formatted and
              exit with code 1 if there are any
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
                package: None,
                emit: None
            })
        );
        assert_eq!(
            p(&["check", "--emit", "ir", "a.brim"]),
            Ok(Command::Check {
                input: Some("a.brim".into()),
                package: None,
                emit: Some(Emit::Ir)
            })
        );
        assert_eq!(
            p(&["check", "--emit=ast"]),
            Ok(Command::Check {
                input: None,
                package: None,
                emit: Some(Emit::Ast)
            })
        );
        assert_eq!(
            p(&["run"]),
            Ok(Command::Run {
                input: None,
                package: None,
                args: vec![]
            })
        );
        assert_eq!(
            p(&["run", "--", "-weird.brim"]),
            Ok(Command::Run {
                input: Some("-weird.brim".into()),
                package: None,
                args: vec![]
            })
        );
        assert_eq!(
            p(&["run", "tool.brim", "a", "--flag", "-x"]),
            Ok(Command::Run {
                input: Some("tool.brim".into()),
                package: None,
                args: vec!["a".into(), "--flag".into(), "-x".into()]
            })
        );
        assert_eq!(p(&["run", "-h"]), Ok(Command::Help(Some("run".into()))));
        assert_eq!(
            p(&["test", "lib.brim", "parse"]),
            Ok(Command::Test {
                input: Some("lib.brim".into()),
                package: None,
                filter: Some("parse".into()),
            })
        );
        assert_eq!(
            p(&["test"]),
            Ok(Command::Test {
                input: None,
                package: None,
                filter: None,
            })
        );
        assert_eq!(
            p(&["fmt", "--check", "src", "a.brim"]),
            Ok(Command::Fmt {
                paths: vec!["src".into(), "a.brim".into()],
                check: true,
            })
        );
        assert_eq!(
            p(&["fmt", "-"]),
            Ok(Command::Fmt {
                paths: vec!["-".into()],
                check: false,
            })
        );
        assert_eq!(
            p(&["gen", "ts", "-o", "api.ts", "app"]),
            Ok(Command::GenTs {
                input: Some("app".into()),
                package: None,
                output: Some("api.ts".into()),
                angular: false,
            })
        );
        assert_eq!(
            p(&["doc", "--std", "-o", "out"]),
            Ok(Command::Doc {
                input: None,
                package: None,
                output: Some("out".into()),
                std: true,
            })
        );
        assert_eq!(
            p(&["gen", "ts", "--angular"]),
            Ok(Command::GenTs {
                input: None,
                package: None,
                output: None,
                angular: true,
            })
        );
        assert_eq!(p(&["lsp", "--stdio"]), Ok(Command::Lsp));
        assert_eq!(
            p(&["build", "-p", "server", "--release"]),
            Ok(Command::Build {
                input: None,
                package: Some("server".into()),
                output: None,
                release: true,
            })
        );
        assert_eq!(
            p(&["publish", "--dry-run", "--package=lib"]),
            Ok(Command::Publish {
                input: None,
                package: Some("lib".into()),
                dry_run: true,
            })
        );
        assert_eq!(
            p(&["login", "--token", "brim_x"]),
            Ok(Command::Login {
                token: Some("brim_x".into())
            })
        );
        assert_eq!(
            p(&["yank", "router@1.2.0", "--undo"]),
            Ok(Command::Yank {
                name: "router".into(),
                version: "1.2.0".into(),
                undo: true,
            })
        );
        assert_eq!(
            p(&["new", "hello"]),
            Ok(Command::New {
                path: "hello".into()
            })
        );
        assert_eq!(
            p(&["build", "--release", "-o", "out/app", "a.brim"]),
            Ok(Command::Build {
                input: Some("a.brim".into()),
                package: None,
                output: Some("out/app".into()),
                release: true,
            })
        );
        assert_eq!(
            p(&["build", "--output=app"]),
            Ok(Command::Build {
                input: None,
                package: None,
                output: Some("app".into()),
                release: false,
            })
        );
    }

    #[test]
    fn usage_errors() {
        let msg = |args: &[&str]| p(args).unwrap_err().message;
        assert_eq!(
            msg(&["frobnicate"]),
            "unknown command `frobnicate` (available: check, run, build, test, new, fmt, update, publish, login, yank, gen, doc, lsp, help, version)"
        );
        assert_eq!(
            msg(&["check", "a.brim", "b.brim"]),
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
        assert_eq!(
            msg(&["run", "--release"]),
            "unknown option `--release` for `brim run`"
        );
        assert_eq!(
            msg(&["test", "a.brim", "x", "y"]),
            "unexpected argument `y`"
        );
        assert_eq!(
            msg(&["new"]),
            "`brim new` needs the path of the project to create"
        );
        assert_eq!(msg(&["new", "a", "b"]), "unexpected argument `b`");
        assert_eq!(msg(&["gen"]), "`brim gen` needs a target: ts");
        assert_eq!(
            msg(&["yank", "router"]),
            "`brim yank` needs NAME@VERSION, e.g. `brim yank router@1.2.0`"
        );
        assert_eq!(
            msg(&["new", "-p", "x"]),
            "unknown option `-p` for `brim new`"
        );
        assert_eq!(
            msg(&["publish", "--release"]),
            "unknown option `--release` for `brim publish`"
        );
        assert_eq!(
            msg(&["gen", "rust"]),
            "unknown target `rust` for `brim gen` (available: ts)"
        );
        assert_eq!(
            msg(&["gen", "ts", "--release"]),
            "unknown option `--release` for `brim gen`"
        );
        assert_eq!(
            msg(&["fmt", "--emit"]),
            "unknown option `--emit` for `brim fmt`"
        );
        assert_eq!(
            msg(&["build", "-o"]),
            "`-o` needs a value: the executable's path"
        );
    }
}
