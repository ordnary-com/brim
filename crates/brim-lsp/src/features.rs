//! Completion, references, rename, signature help and quick fixes.
//!
//! References and rename use the checker's index of what each name refers
//! to. Completion after `value.` asks the checker for the value's type: the
//! file is checked again without the `.` being typed, which leaves a
//! program that parses.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use brim_ast::{FnDecl, Program};
use brim_diagnostics::{Diagnostic, Sources, Span};
use brim_lexer::TokenKind;
use brim_semantic::checked::{Member, MemberKind};
use serde_json::{Value, json};

use crate::analysis::{self, Analysis};

/// LSP `CompletionItemKind`s.
const METHOD: u32 = 2;
const FUNCTION: u32 = 3;
const FIELD: u32 = 5;
const VARIABLE: u32 = 6;
const MODULE: u32 = 9;
const ENUM: u32 = 13;
const KEYWORD: u32 = 14;
const ENUM_MEMBER: u32 = 20;
const CONSTANT: u32 = 21;
const STRUCT: u32 = 22;
const TYPE: u32 = 25;

/// Functions that are built in but not in `brim_stdlib::BUILTINS`.
const SPECIAL_FUNCTIONS: &[(&str, &str)] = &[
    ("read_line", "fn read_line() -> Option[string]"),
    (
        "read_file",
        "fn read_file(path: string) -> Result[string, string]",
    ),
    (
        "write_file",
        "fn write_file(path: string, text: string) -> Result[i64, string]",
    ),
    ("args", "fn args() -> List[string]"),
    ("env", "fn env(name: string) -> Option[string]"),
    (
        "string_from_bytes",
        "fn string_from_bytes(bytes: List[u8]) -> Option[string]",
    ),
    (
        "from_code_point",
        "fn from_code_point(n: i64) -> Option[string]",
    ),
    ("to_i32", "fn to_i32(n: i64) -> i32"),
    ("to_u8", "fn to_u8(n: i64) -> u8"),
    ("to_u64", "fn to_u64(n: i64) -> u64"),
    (
        "serve_api",
        "fn serve_api(module, request: http.Request) -> http.Response",
    ),
];

const TYPES: &[&str] = &[
    "i64", "i32", "u8", "u64", "f64", "bool", "string", "void", "List", "Map", "Set", "Option",
    "Result", "Box",
];

fn item(label: &str, kind: u32, detail: &str) -> Value {
    json!({ "label": label, "kind": kind, "detail": detail })
}

fn member_item(m: &Member) -> Value {
    let kind = match m.kind {
        MemberKind::Field => FIELD,
        MemberKind::Method => METHOD,
        MemberKind::Variant => ENUM_MEMBER,
    };
    item(&m.name, kind, &m.detail)
}

/// The start of the identifier that ends at `offset`.
fn word_start(text: &str, offset: usize) -> usize {
    let before = &text[..offset];
    before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(offset, |(i, _)| i)
}

/// Completion items at byte `offset` of `path`, whose text is in `open`.
pub(crate) fn completion(path: &Path, open: &HashMap<PathBuf, String>, offset: usize) -> Value {
    let text = open
        .get(path)
        .cloned()
        .or_else(|| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let offset = offset.min(text.len());
    let start = word_start(&text, offset);
    let analysis = analysis::analyze(path, open);
    let Some(module) = analysis.file_index(path) else {
        return json!([]);
    };
    let base = analysis.map().base(module);
    let program = &analysis.loaded.modules[module].program;

    if text[..start].ends_with('.') {
        let dot = start - 1;
        let base_start = word_start(&text, dot);
        let base_name = &text[base_start..dot];
        let after_dot = text[..base_start].ends_with('.');
        if !base_name.is_empty() && !after_dot {
            if let Some(target) = analysis.module_alias_index(program, base_name) {
                return Value::Array(module_items(&analysis, target, true));
            }
            let is_variable = analysis
                .local_names(program, base + dot)
                .iter()
                .any(|(n, _)| n == base_name);
            if !is_variable && let Some(members) = analysis.index.type_members.get(base_name) {
                return Value::Array(members.iter().map(member_item).collect());
            }
        }
        // A value: check the file without `.word` and ask for the type of
        // the expression that ends where the dot was.
        let word_end = offset
            + text[offset..]
                .char_indices()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map_or(text.len() - offset, |(i, _)| i);
        let mut patched = open.clone();
        let mut without = text[..dot].to_string();
        without.push_str(&text[word_end..]);
        patched.insert(path.to_path_buf(), without);
        let checked = analysis::analyze(path, &patched);
        let Some(module) = checked.file_index(path) else {
            return json!([]);
        };
        let end = checked.map().base(module) + dot;
        let ty = checked
            .index
            .types
            .iter()
            .filter(|(span, _)| span.end == end)
            .max_by_key(|(span, _)| span.end - span.start)
            .map(|(_, ty)| *ty);
        let members = ty.and_then(|ty| checked.index.members.get(&ty));
        return Value::Array(members.map_or(Vec::new(), |m| m.iter().map(member_item).collect()));
    }

    let mut items: Vec<Value> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |items: &mut Vec<Value>, label: &str, kind: u32, detail: &str| {
        if seen.insert(label.to_string()) {
            items.push(item(label, kind, detail));
        }
    };
    for (name, detail) in analysis.local_names(program, base + offset) {
        add(&mut items, &name, VARIABLE, &detail);
    }
    for v in module_items(&analysis, module, false) {
        let label = v["label"].as_str().unwrap_or_default().to_string();
        let kind = v["kind"].as_u64().unwrap_or(0) as u32;
        let detail = v["detail"].as_str().unwrap_or_default().to_string();
        add(&mut items, &label, kind, &detail);
    }
    for import in &program.imports {
        match &import.items {
            Some(names) => {
                if let Some(target) = analysis.module_named(&joined(&import.path)) {
                    for v in module_items(&analysis, target, true) {
                        if names.iter().any(|n| v["label"] == json!(n.name)) {
                            let label = v["label"].as_str().unwrap_or_default().to_string();
                            let kind = v["kind"].as_u64().unwrap_or(0) as u32;
                            let detail = v["detail"].as_str().unwrap_or_default().to_string();
                            add(&mut items, &label, kind, &detail);
                        }
                    }
                }
            }
            None => {
                let alias = import.alias.as_ref().or(import.path.last());
                if let Some(alias) = alias {
                    add(
                        &mut items,
                        &alias.name,
                        MODULE,
                        &format!("module {}", joined(&import.path)),
                    );
                }
            }
        }
    }
    // The prelude's functions and constants, other than methods.
    if let Some(prelude) = analysis.module_named("std.prelude") {
        for v in module_items(&analysis, prelude, true) {
            let label = v["label"].as_str().unwrap_or_default().to_string();
            let is_method = ["list_", "set_", "string_", "option_", "result_"]
                .iter()
                .any(|p| label.starts_with(p));
            if !is_method {
                let kind = v["kind"].as_u64().unwrap_or(0) as u32;
                let detail = v["detail"].as_str().unwrap_or_default().to_string();
                add(&mut items, &label, kind, &detail);
            }
        }
    }
    for b in brim_stdlib::BUILTINS {
        if !brim_stdlib::is_internal(b.name) {
            add(&mut items, b.name, FUNCTION, b.summary);
        }
    }
    for (name, detail) in SPECIAL_FUNCTIONS {
        add(&mut items, name, FUNCTION, detail);
    }
    for name in TYPES {
        add(&mut items, name, TYPE, "built-in type");
    }
    for (word, _) in brim_lexer::KEYWORDS {
        add(&mut items, word, KEYWORD, "keyword");
    }
    Value::Array(items)
}

/// The items of a module: functions, structs, enums and constants; only
/// the `pub` ones for another module.
fn module_items(analysis: &Analysis, module: usize, public_only: bool) -> Vec<Value> {
    let program = &analysis.loaded.modules[module].program;
    let mut out = Vec::new();
    for f in program
        .functions
        .iter()
        .filter(|f| f.public || !public_only)
    {
        out.push(item(&f.name.name, FUNCTION, &analysis.fn_header(f)));
    }
    for s in program.structs.iter().filter(|s| s.public || !public_only) {
        out.push(item(
            &s.name.name,
            STRUCT,
            &format!("struct {}", s.name.name),
        ));
    }
    for e in program.enums.iter().filter(|e| e.public || !public_only) {
        out.push(item(&e.name.name, ENUM, &format!("enum {}", e.name.name)));
    }
    for c in program.consts.iter().filter(|c| c.public || !public_only) {
        out.push(item(&c.name.name, CONSTANT, &analysis.span_text(c.span)));
    }
    out
}

fn joined(path: &[brim_ast::Ident]) -> String {
    path.iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

// ---- references and rename ----------------------------------------------

impl Analysis {
    /// The declaration the name at global offset `at` refers to, or the
    /// declaration whose name is at `at`.
    pub(crate) fn target(&self, module: usize, at: usize) -> Option<Span> {
        let inside = |s: &Span| s.start <= at && at <= s.end;
        if let Some((_, decl)) = self.index.refs.iter().find(|(used, _)| inside(used)) {
            return Some(*decl);
        }
        if let Some(decl) = self.index.refs.iter().map(|(_, d)| *d).find(inside) {
            return Some(decl);
        }
        // A declaration nothing uses yet: one the syntax index knows.
        self.declarations(module, at)
            .into_iter()
            .map(|d| d.span)
            .find(inside)
    }

    /// Every use of the declaration at `decl`, and the declaration itself,
    /// in order.
    pub(crate) fn references(&self, decl: Span) -> Vec<Span> {
        let mut spans: Vec<Span> = self
            .index
            .refs
            .iter()
            .filter(|(_, d)| *d == decl)
            .map(|(used, _)| *used)
            .collect();
        spans.push(decl);
        spans.sort_by_key(|s| (s.start, s.end));
        spans.dedup();
        spans
    }
}

/// The text edits of a rename, by file: each reference's name (the last
/// part of `module.Name`) becomes `new_name`.
pub(crate) fn rename_edits(
    analysis: &Analysis,
    decl: Span,
    new_name: &str,
) -> Result<BTreeMap<String, Vec<(usize, usize)>>, String> {
    let ident = !new_name.is_empty()
        && new_name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && new_name.chars().all(|c| c.is_alphanumeric() || c == '_');
    let is_keyword = brim_lexer::KEYWORDS.iter().any(|(w, _)| *w == new_name)
        || brim_lexer::RESERVED.contains(&new_name);
    if !ident || is_keyword {
        return Err(format!("`{new_name}` is not a valid name"));
    }
    let (decl_file, _, _) = analysis.map().locate(decl.start);
    if !Path::new(decl_file.name()).is_file() {
        return Err("names of the standard library cannot be renamed".into());
    }
    let mut out: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for span in analysis.references(decl) {
        let (file, base, _) = analysis.map().locate(span.start);
        let text = &file.text()[span.start - base..span.end - base];
        // `module.Name`: only the name.
        let start = span.start - base + text.rfind('.').map_or(0, |i| i + 1);
        out.entry(file.name().to_string())
            .or_default()
            .push((start, span.end - base));
    }
    Ok(out)
}

// ---- signature help -------------------------------------------------------

/// The signature of the call around byte `offset`: its label, the ranges
/// of its parameters within the label and the active parameter.
pub(crate) fn signature_help(analysis: &Analysis, module: usize, offset: usize) -> Value {
    let base = analysis.map().base(module);
    let text = analysis.map().files()[module].text();
    let (tokens, _) = brim_lexer::lex(text);
    // The innermost `(` before the cursor that is still open.
    let mut depth = 0i32;
    let mut commas = 0;
    let mut open = None;
    for (i, t) in tokens.iter().enumerate().rev() {
        if t.span.end > offset {
            continue;
        }
        match t.kind {
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => depth += 1,
            TokenKind::LParen if depth == 0 => {
                open = Some(i);
                break;
            }
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                if depth == 0 {
                    return Value::Null;
                }
                depth -= 1;
            }
            TokenKind::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let Some(open) = open.filter(|&i| i > 0) else {
        return Value::Null;
    };
    let callee = &tokens[open - 1];
    if !matches!(callee.kind, TokenKind::Ident(_)) {
        return Value::Null;
    }
    let is_method = open >= 2 && tokens[open - 2].kind == TokenKind::Dot;
    let found = analysis.declarations(module, base + callee.span.start);
    let Some(decl) = found.into_iter().find(|d| d.detail.contains("fn ")) else {
        return Value::Null;
    };
    let label = decl.detail.clone();
    let mut params = parameter_ranges(&label);
    // A method call passes `self` before the parentheses.
    if is_method
        && let Some((start, end)) = params.first().copied()
        && label[start..end]
            .trim_start_matches("mut ")
            .starts_with("self")
    {
        params.remove(0);
    }
    let parameters: Vec<Value> = params
        .iter()
        .map(|(s, e)| json!({ "label": [utf16_len(&label[..*s]), utf16_len(&label[..*e])] }))
        .collect();
    json!({
        "signatures": [{ "label": label, "parameters": parameters }],
        "activeSignature": 0,
        "activeParameter": commas.min(params.len().saturating_sub(1)),
    })
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte ranges of the parameters in `fn name(a: A, b: B) -> R`.
fn parameter_ranges(label: &str) -> Vec<(usize, usize)> {
    let Some(open) = label.find('(') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = open + 1;
    for (i, c) in label[open..].char_indices() {
        let i = open + i;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 1 => depth -= 1,
            ')' => {
                if label[start..i].trim().is_empty() {
                    return out;
                }
                out.push(trimmed(label, start, i));
                return out;
            }
            ',' if depth == 1 => {
                out.push(trimmed(label, start, i));
                start = i + 1;
            }
            _ => {}
        }
    }
    out
}

fn trimmed(text: &str, start: usize, end: usize) -> (usize, usize) {
    let part = &text[start..end];
    let lead = part.len() - part.trim_start().len();
    let trail = part.len() - part.trim_end().len();
    (start + lead, end - trail)
}

// ---- quick fixes ----------------------------------------------------------

/// The fixes a diagnostic suggests that can be applied as they are, as
/// `{ title, edits: [{ start, end, text }] }` with byte offsets into its
/// file.
pub(crate) fn fixes(diag: &Diagnostic, analysis: &Analysis) -> Vec<Value> {
    let Some(primary) = &diag.primary else {
        return Vec::new();
    };
    let (file, base, module) = analysis.map().locate(primary.span.start);
    let text = file.text();
    let (start, end) = (primary.span.start - base, primary.span.end - base);
    let mut out = Vec::new();
    let edit = |s: usize, e: usize, t: &str| json!({ "start": s, "end": e, "text": t });
    for help in &diag.help {
        let similar = between(help, "did you mean `", "`?").or_else(|| {
            help.split_once("with a similar name exists: `")
                .and_then(|(_, rest)| rest.strip_suffix('`'))
        });
        if let Some(name) = similar {
            // Replace the name at the end of what is underlined.
            let shown = &text[start..end];
            let name_start = start + shown.rfind('.').map_or(0, |i| i + 1);
            out.push(json!({
                "title": format!("Change to `{name}`"),
                "edits": [edit(name_start, end, name)],
            }));
        } else if help.starts_with("make it mutable") {
            for label in &diag.secondary {
                let decl = label.span.start - base;
                if label.span.start >= base && text[..decl].trim_end().ends_with("let") {
                    out.push(json!({
                        "title": "Make it mutable",
                        "edits": [edit(decl, decl, "mut ")],
                    }));
                }
            }
        } else if let Some(import) = between(help, "import it first: `", "`")
            .or_else(|| between(help, "add `", "`").filter(|s| s.starts_with("import ")))
        {
            let at = import_position(&analysis.loaded.modules[module].program, base, text);
            out.push(json!({
                "title": format!("Add `{import}`"),
                "edits": [edit(at, at, &format!("{import}\n"))],
            }));
        } else if let Some(written) = between(help, "write `", "` for a floating-point number") {
            out.push(json!({
                "title": format!("Write `{written}`"),
                "edits": [edit(start, end, written)],
            }));
        } else if let Some(written) = between(help, "write just `", "`") {
            out.push(json!({
                "title": format!("Write `{written}`"),
                "edits": [edit(start, end, written)],
            }));
        }
    }
    out
}

/// The text between `before` and the next `after` in `text`, when `text`
/// starts with `before`.
fn between<'t>(text: &'t str, before: &str, after: &str) -> Option<&'t str> {
    let rest = text.strip_prefix(before)?;
    rest.find(after).map(|i| &rest[..i])
}

/// Where a new import goes: after the last import, or at the start.
fn import_position(program: &Program, base: usize, text: &str) -> usize {
    match program.imports.iter().map(|i| i.span.end).max() {
        Some(end) => {
            let end = end - base;
            text[end..].find('\n').map_or(text.len(), |i| end + i + 1)
        }
        None => 0,
    }
}

// ---- helpers on the analysis ---------------------------------------------

impl Analysis {
    pub(crate) fn span_text(&self, span: Span) -> String {
        let (file, base, _) = self.map().locate(span.start);
        file.text()[span.start - base..span.end - base].to_string()
    }

    /// `pub fn name(params) -> ret`.
    pub(crate) fn fn_header(&self, f: &FnDecl) -> String {
        let header = Span::new(f.span.start, f.body.span.start);
        let text = self.span_text(header);
        let text = text.trim_end();
        if f.public && !text.starts_with("pub ") {
            format!("pub {text}")
        } else {
            text.to_string()
        }
    }
}
