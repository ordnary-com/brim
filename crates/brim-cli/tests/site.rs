//! The examples on the documentation site (`site/src`) run and print what
//! the page says: a ```brim block with `fn main` must run, and a ```text
//! block right after it is its expected output. Blocks that use other
//! modules are skipped. `BLESS=1` updates the outputs.

use std::fs;
use std::path::Path;
use std::process::Command;

struct Block {
    lang: String,
    /// Byte range of the contents in the page.
    start: usize,
    end: usize,
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut open: Option<(String, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if let Some(rest) = trimmed.strip_prefix("```") {
            match open.take() {
                None => open = Some((rest.trim().to_string(), offset + line.len())),
                Some((lang, start)) => out.push(Block {
                    lang,
                    start,
                    end: offset,
                }),
            }
        }
        offset += line.len();
    }
    out
}

#[test]
fn site_examples_run() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("site");
    let _ = fs::create_dir_all(&dir);
    let bless = std::env::var_os("BLESS").is_some();
    let mut failures = Vec::new();
    let mut ran = 0;
    for page in fs::read_dir(root.join("site/src")).unwrap() {
        let path = page.unwrap().path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let mut text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let found = blocks(&text);
        let mut edits = Vec::new();
        for (i, block) in found.iter().enumerate() {
            let code = &text[block.start..block.end];
            if block.lang != "brim" || !code.contains("fn main") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            // Programs that use the network or call C are checked, not run.
            if code.lines().any(|l| l.starts_with("use @std/")) || code.contains("extern fn") {
                let file = dir.join(format!("example{ran}.brim"));
                fs::write(&file, code).unwrap();
                ran += 1;
                let out = Command::new(env!("CARGO_BIN_EXE_brim"))
                    .args(["check", file.to_str().unwrap()])
                    .env("NO_COLOR", "1")
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() || stderr.contains("warning") {
                    failures.push(format!(
                        "{name}: example does not check cleanly:\n{code}\n{stderr}"
                    ));
                }
                continue;
            }
            if code
                .lines()
                .any(|l| l.starts_with("use ") || l.starts_with("pub use "))
            {
                continue;
            }
            let file = dir.join(format!("example{ran}.brim"));
            fs::write(&file, code).unwrap();
            ran += 1;
            let out = Command::new(env!("CARGO_BIN_EXE_brim"))
                .args(["run", file.to_str().unwrap()])
                .env("NO_COLOR", "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.status.success() {
                failures.push(format!(
                    "{name}: example fails:\n{code}\n{}",
                    String::from_utf8_lossy(&out.stderr)
                ));
                continue;
            }
            let Some(next) = found.get(i + 1) else {
                continue;
            };
            let between = &text[block.end..next.start];
            let adjacent = between.lines().filter(|l| !l.trim().is_empty()).count() <= 2;
            if next.lang != "text" || !adjacent {
                continue;
            }
            let expected = &text[next.start..next.end];
            if expected != stdout {
                if bless {
                    edits.push((next.start, next.end, stdout));
                } else {
                    failures.push(format!(
                        "{name}: output differs for\n{code}\n--- page\n{expected}--- actual\n{stdout}"
                    ));
                }
            }
        }
        if !edits.is_empty() {
            for (start, end, new) in edits.into_iter().rev() {
                text.replace_range(start..end, &new);
            }
            fs::write(&path, text).unwrap();
        }
    }
    assert!(ran >= 10, "only {ran} examples found");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
