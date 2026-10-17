//! The Brim interpreter for the browser: runs programs with [`run`],
//! compiled to WebAssembly (`wasm32-wasip1`).
//!
//! A program gets the text typed as its standard input, can import only
//! `std` modules, and stops with the run-time error `R0009` after
//! [`MAX_STEPS`] steps, so a loop that never ends does not hang the page.
//! The output is the same as `brim run` gives for a file `main.brim`.

#[cfg(target_family = "wasm")]
mod wasm;

/// How many instructions a program may run: a few seconds of work.
pub const MAX_STEPS: u64 = 200_000_000;

/// What running a program printed, and its exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Checks and runs `source` as `main.brim`, with `input` as standard input.
pub fn run(source: &str, input: &str, max_steps: u64) -> Output {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::Cursor::new(input.as_bytes().to_vec());
    let code = {
        let mut ctx = brim_cli::Context {
            cwd: std::path::PathBuf::from("/"),
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin: &mut stdin,
            color: brim_diagnostics::ColorChoice::Never,
        };
        brim_cli::run_source("main.brim", source, Some(max_steps), &mut ctx)
    };
    Output {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        code,
    }
}

/// The result of [`run`] as JSON: `{"stdout": ..., "stderr": ..., "code": ...}`.
pub fn run_json(source: &str, input: &str, max_steps: u64) -> String {
    let out = run(source, input, max_steps);
    serde_json::json!({ "stdout": out.stdout, "stderr": out.stderr, "code": out.code }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_programs() {
        let out = run(
            "fn main() {\n    print(read_line() ?? \"\")\n}\n",
            "hi\n",
            MAX_STEPS,
        );
        assert_eq!(out.code, 1, "{out:?}");
        let out = run(
            "fn main() {\n    match read_line() {\n        Some(l) => print(l)\n        None => {}\n    }\n}\n",
            "hi\n",
            MAX_STEPS,
        );
        assert_eq!(
            out,
            Output {
                stdout: "hi\n".into(),
                stderr: String::new(),
                code: 0
            }
        );
    }

    #[test]
    fn errors_and_limits() {
        let out = run("fn main() {\n    let x: i64 = \"no\"\n}\n", "", MAX_STEPS);
        assert_eq!(out.code, 1);
        assert!(
            out.stderr.contains("error[E0206]: mismatched types"),
            "{}",
            out.stderr
        );
        assert!(out.stderr.contains("--> main.brim:2:18"), "{}", out.stderr);
        let out = run("fn main() {\n    loop {}\n}\n", "", 10_000);
        assert_eq!(out.code, 101);
        assert!(out.stderr.contains("R0009"), "{}", out.stderr);
        let json = run_json("fn main() {\n    print(1)\n}\n", "", MAX_STEPS);
        assert_eq!(json, r#"{"code":0,"stderr":"","stdout":"1\n"}"#);
    }
}
