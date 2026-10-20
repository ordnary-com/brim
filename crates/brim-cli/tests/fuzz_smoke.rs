//! A deterministic mutation fuzzer that runs with the other tests.
//!
//! It mutates the example and test programs (deleting, repeating and
//! inserting text, swapping lines) and feeds every mutant through the
//! frontend. No stage may panic, a checked program must lower to valid IR,
//! and the formatter must accept every program that parses and be
//! idempotent. `fuzz/` has cargo-fuzz targets for longer runs.
//!
//! `BRIM_FUZZ_ITERATIONS` sets the number of mutants (default 3000) and
//! `BRIM_FUZZ_SEED` the random seed.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// xorshift64*: small, fast and good enough to pick mutations.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const FRAGMENTS: &[&str] = &[
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    ",",
    ":",
    ";",
    ".",
    "..",
    "?",
    "=>",
    "->",
    "=",
    "==",
    "+",
    "-",
    "*",
    "/",
    "%",
    "!",
    "&&",
    "||",
    "<",
    "<=",
    "\n",
    "\"",
    "\"text\"",
    "0",
    "1",
    "9223372036854775807",
    "99999999999999999999999",
    "x",
    "self",
    "fn",
    "let",
    "let mut x: i64 = ",
    "return",
    "if",
    "else",
    "while",
    "for",
    "in",
    "match",
    "struct",
    "enum",
    "impl",
    "pub",
    "import a",
    "use @a/b",
    "as",
    "_",
    "Some(",
    "None",
    "Ok(",
    "Err(",
    "Option[",
    "List[",
    "Map[",
    "i64",
    "bool",
    "string",
    "void",
    "T",
    "[T]",
    "[:]",
    "break",
    "continue",
    "print(",
    "assert_eq(",
    "é",
    "\\n",
    "//",
    "async",
];

fn corpus() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = ["examples", "tests"].iter().map(|d| root.join(d)).collect();
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else if path.extension().is_some_and(|e| e == "brim")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            files.push(text);
        }
    }
    files.sort();
    files
}

/// A byte position on a character boundary.
fn boundary(text: &str, rng: &mut Rng) -> usize {
    let mut i = rng.below(text.len() + 1);
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn mutate(text: &str, rng: &mut Rng) -> String {
    let mut out = text.to_string();
    for _ in 0..=rng.below(3) {
        let a = boundary(&out, rng);
        let b = boundary(&out, rng);
        let (a, mut b) = (a.min(b), a.max(b));
        // At most 40 bytes, ending on a character boundary.
        b = b.min(a + 40);
        while !out.is_char_boundary(b) {
            b -= 1;
        }
        match rng.below(5) {
            0 => out.replace_range(a..b, ""),
            1 => {
                let piece = out[a..b].to_string();
                out.insert_str(a, &piece);
            }
            2 | 3 => out.insert_str(a, FRAGMENTS[rng.below(FRAGMENTS.len())]),
            _ => {
                let mut lines: Vec<&str> = out.lines().collect();
                if lines.len() > 1 {
                    let i = rng.below(lines.len());
                    let j = rng.below(lines.len());
                    lines.swap(i, j);
                }
                out = lines.join("\n");
            }
        }
    }
    out
}

/// How far mutants got.
#[derive(Default)]
struct Stats {
    parsed: usize,
    checked: usize,
    ran: usize,
}

/// Runs a program with the interpreter, limited to a number of steps, in a
/// scratch directory so that programs writing files stay there. Run-time
/// errors are fine; panics are not.
fn run(module: &brim_ir::Module, stats: &mut Stats) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz-cwd");
    let _ = std::fs::create_dir_all(&dir);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut host = brim_interp::Host {
        out: &mut out,
        err: &mut err,
        input: &mut std::io::empty(),
        args: vec!["arg".into()],
        cwd: dir,
    };
    let limits = brim_interp::Limits {
        max_call_depth: 200,
        max_steps: Some(20_000),
    };
    let _ = brim_interp::run(module, &mut host, limits);
    stats.ran += 1;
}

/// Runs the frontend on `src`; returns a description of a broken invariant.
fn check(src: &str, stats: &mut Stats) -> Option<String> {
    let (tokens, _) = brim_lexer::lex(src);
    if tokens.last().map(|t| &t.kind) != Some(&brim_lexer::TokenKind::Eof) {
        return Some("the token stream does not end with Eof".into());
    }
    let (ast, diags) = brim_parser::parse(src);
    let parsed = !brim_diagnostics::has_errors(&diags);
    for d in &diags {
        if let Some(span) = d.span()
            && span.end > src.len()
        {
            return Some(format!(
                "diagnostic span {span:?} beyond the text: {}",
                d.message
            ));
        }
    }
    // Rendering must not panic either.
    let file = brim_diagnostics::SourceFile::new("fuzz.brim", src);
    let _ = brim_diagnostics::Renderer::new(brim_diagnostics::ColorChoice::Never)
        .render_all(&diags, &file);

    match brim_fmt::format(src) {
        Ok(formatted) => {
            if !parsed {
                return Some("the formatter accepted a program with syntax errors".into());
            }
            match brim_fmt::format(&formatted) {
                Ok(again) if again == formatted => {}
                Ok(_) => return Some("formatting is not idempotent".into()),
                Err(d) => return Some(format!("formatted text does not format: {d:?}")),
            }
        }
        Err(d) if parsed => return Some(format!("the formatter rejected a valid program: {d:?}")),
        Err(_) => {}
    }
    if !parsed {
        return None;
    }
    stats.parsed += 1;
    let modules = [brim_semantic::ModuleAst {
        name: String::new(),
        program: ast,
    }];
    for tests in [false, true] {
        let (checked, diags) = if tests {
            brim_semantic::check_tests(&modules)
        } else {
            brim_semantic::check_modules(&modules)
        };
        let _ = brim_diagnostics::Renderer::new(brim_diagnostics::ColorChoice::Never)
            .render_all(&diags, &file);
        if let Some(checked) = checked {
            stats.checked += usize::from(tests);
            let (module, _) = brim_lower::lower_tests(&checked);
            if let Err(errors) = brim_ir::validate(&module) {
                return Some(format!("invalid IR: {errors:?}"));
            }
            if !tests {
                run(&module, stats);
            }
        }
    }
    None
}

#[test]
fn mutated_programs_do_not_break_the_frontend() {
    let iterations: usize = std::env::var("BRIM_FUZZ_ITERATIONS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let seed: u64 = std::env::var("BRIM_FUZZ_SEED")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0x5eed_b41f);
    let corpus = corpus();
    assert!(corpus.len() > 20);
    let mut rng = Rng(seed.wrapping_mul(2).wrapping_add(1));
    panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    let mut stats = Stats::default();
    for _ in 0..iterations {
        let base = &corpus[rng.below(corpus.len())];
        let src = mutate(base, &mut rng);
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| check(&src, &mut stats)));
        let problem = match outcome {
            Ok(None) => continue,
            Ok(Some(problem)) => problem,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                format!("panic: {message}")
            }
        };
        failures.push((problem, src));
        if failures.len() >= 5 {
            break;
        }
    }
    let _ = panic::take_hook();
    eprintln!(
        "{iterations} mutants: {} parse, {} pass semantic analysis, {} run",
        stats.parsed, stats.checked, stats.ran
    );
    if let Some((_, smallest)) = failures.iter().min_by_key(|(_, s)| s.len()) {
        let list: Vec<String> = failures.iter().map(|(p, _)| p.clone()).collect();
        eprintln!("{}\n----\n{smallest}\n----", list.join("\n"));
        panic!(
            "{} failing input(s):\n{}\n\nsmallest input:\n----\n{smallest}\n----",
            failures.len(),
            list.join("\n")
        );
    }
}
