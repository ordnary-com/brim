//! The Brim language server (`brim lsp`).
//!
//! It speaks the Language Server Protocol over standard input and output
//! and offers:
//!
//! - diagnostics, published when a file is opened, changed or saved, for
//!   every file of the program (with the same messages as `brim check`);
//! - formatting (`brim fmt`);
//! - an outline of each file (document symbols);
//! - go to definition and hover for variables, parameters, functions,
//!   types, fields, variants, methods and modules.
//!
//! Documents are synchronised in full on every change; Brim files are
//! small, and the whole program is checked again anyway.

mod analysis;
mod protocol;

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use brim_diagnostics::{Severity, Sources};
use serde_json::{Value, json};

use analysis::{Analysis, Symbol};
use protocol::{offset, path_to_uri, range, uri_to_path};

/// Serves one client until it sends `exit` or closes the input. Returns
/// the process exit code: 0 after `shutdown`, 1 otherwise.
pub fn run(input: &mut dyn BufRead, output: &mut dyn Write) -> io::Result<i32> {
    let mut server = Server {
        open: HashMap::new(),
        uris: HashMap::new(),
        published: HashSet::new(),
        shutdown: false,
    };
    while let Some(message) = protocol::read_message(input)? {
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let id = message.get("id").cloned();
        if method == "exit" {
            return Ok(if server.shutdown { 0 } else { 1 });
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match id {
            Some(id) if !method.is_empty() => {
                let response = match server.request(&method, &params) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err((code, text)) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": code, "message": text },
                    }),
                };
                protocol::write_message(output, &response)?;
            }
            // A response to a request of ours: there are none.
            Some(_) => {}
            None => {
                for notification in server.notify(&method, &params) {
                    protocol::write_message(output, &notification)?;
                }
            }
        }
    }
    Ok(1)
}

struct Server {
    /// The texts of the open files.
    open: HashMap<PathBuf, String>,
    /// The URIs the client uses for the open files.
    uris: HashMap<PathBuf, String>,
    /// Files with published diagnostics, to clear them when they are gone.
    published: HashSet<String>,
    shutdown: bool,
}

type RequestResult = Result<Value, (i64, String)>;

impl Server {
    fn request(&mut self, method: &str, params: &Value) -> RequestResult {
        match method {
            "initialize" => Ok(json!({
                "capabilities": {
                    "textDocumentSync": { "openClose": true, "change": 1, "save": true },
                    "documentFormattingProvider": true,
                    "documentSymbolProvider": true,
                    "definitionProvider": true,
                    "hoverProvider": true,
                },
                "serverInfo": { "name": "brim", "version": env!("CARGO_PKG_VERSION") },
            })),
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "textDocument/formatting" => self.formatting(params),
            "textDocument/documentSymbol" => self.document_symbols(params),
            "textDocument/definition" => self.definition(params),
            "textDocument/hover" => self.hover(params),
            _ => Err((-32601, format!("unsupported method `{method}`"))),
        }
    }

    fn notify(&mut self, method: &str, params: &Value) -> Vec<Value> {
        let doc = &params["textDocument"];
        let Some(uri) = doc["uri"].as_str() else {
            return Vec::new();
        };
        let Some(path) = uri_to_path(uri) else {
            return Vec::new();
        };
        match method {
            "textDocument/didOpen" => {
                let text = doc["text"].as_str().unwrap_or_default().to_string();
                self.open.insert(path.clone(), text);
                self.uris.insert(path.clone(), uri.to_string());
            }
            "textDocument/didChange" => {
                // Full synchronisation: the last change is the whole text.
                if let Some(text) = params["contentChanges"]
                    .as_array()
                    .and_then(|c| c.last())
                    .and_then(|c| c["text"].as_str())
                {
                    self.open.insert(path.clone(), text.to_string());
                }
            }
            "textDocument/didSave" => {}
            "textDocument/didClose" => {
                self.open.remove(&path);
                self.uris.remove(&path);
            }
            _ => return Vec::new(),
        }
        self.diagnostics(&path)
    }

    fn uri(&self, path: &Path) -> String {
        self.uris
            .get(path)
            .cloned()
            .unwrap_or_else(|| path_to_uri(path))
    }

    /// `publishDiagnostics` for every file of the program `path` is in.
    fn diagnostics(&mut self, path: &Path) -> Vec<Value> {
        let analysis = analysis::analyze(path, &self.open);
        let map = analysis.map();
        let mut by_file: Vec<Vec<Value>> = vec![Vec::new(); map.files().len()];
        for diag in &analysis.diags {
            let (index, value) = match diag.span() {
                Some(span) => {
                    let (file, base, index) = map.locate(span.start);
                    let range = range(file.text(), span.start - base, span.end - base);
                    (index, lsp_diagnostic(diag, range))
                }
                None => (0, lsp_diagnostic(diag, range("", 0, 0))),
            };
            by_file[index].push(value);
        }
        let mut out = Vec::new();
        let mut now = HashSet::new();
        for (file, diags) in map.files().iter().zip(by_file) {
            let uri = self.uri(Path::new(file.name()));
            now.insert(uri.clone());
            out.push(publish(&uri, diags));
        }
        // Clear files that are no longer part of the program.
        for uri in self.published.difference(&now) {
            out.push(publish(uri, Vec::new()));
        }
        self.published = now;
        out
    }

    /// The open file a request is about, with its analysis.
    fn document(&self, params: &Value) -> Result<(PathBuf, Analysis, usize), (i64, String)> {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let path = uri_to_path(uri).ok_or((-32602, format!("not a file URI: {uri}")))?;
        let analysis = analysis::analyze(&path, &self.open);
        let index = analysis
            .file_index(&path)
            .ok_or((-32603, "the file could not be analysed".to_string()))?;
        Ok((path, analysis, index))
    }

    fn formatting(&self, params: &Value) -> RequestResult {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let path = uri_to_path(uri).ok_or((-32602, format!("not a file URI: {uri}")))?;
        let Some(text) = self
            .open
            .get(&path)
            .cloned()
            .or_else(|| std::fs::read_to_string(&path).ok())
        else {
            return Ok(Value::Null);
        };
        match brim_fmt::format(&text) {
            // Files with syntax errors are left alone.
            Err(_) => Ok(Value::Null),
            Ok(formatted) if formatted == text => Ok(json!([])),
            Ok(formatted) => Ok(json!([{
                "range": range(&text, 0, text.len()),
                "newText": formatted,
            }])),
        }
    }

    fn document_symbols(&self, params: &Value) -> RequestResult {
        let (_, analysis, index) = self.document(params)?;
        let file = &analysis.map().files()[index];
        let base = analysis.map().base(index);
        let symbols: Vec<Value> = analysis
            .symbols(index)
            .iter()
            .map(|s| symbol_json(s, file.text(), base))
            .collect();
        Ok(Value::Array(symbols))
    }

    fn declarations_at(
        &self,
        params: &Value,
    ) -> Result<(Analysis, Vec<analysis::Declaration>), (i64, String)> {
        let (_, analysis, index) = self.document(params)?;
        let file = &analysis.map().files()[index];
        let at = analysis.map().base(index) + offset(file.text(), &params["position"]);
        let found = analysis.declarations(index, at);
        Ok((analysis, found))
    }

    fn definition(&self, params: &Value) -> RequestResult {
        let (analysis, found) = self.declarations_at(params)?;
        let locations: Vec<Value> = found
            .iter()
            .map(|d| {
                let (file, base, _) = analysis.map().locate(d.span.start);
                json!({
                    "uri": self.uri(Path::new(file.name())),
                    "range": range(file.text(), d.span.start - base, d.span.end - base),
                })
            })
            .collect();
        Ok(Value::Array(locations))
    }

    fn hover(&self, params: &Value) -> RequestResult {
        let (_, found) = self.declarations_at(params)?;
        let Some(first) = found.first() else {
            return Ok(Value::Null);
        };
        let mut text = format!("```brim\n{}\n```", first.detail);
        if found.len() > 1 {
            text.push_str(&format!("\n\n{} more declarations", found.len() - 1));
        }
        Ok(json!({ "contents": { "kind": "markdown", "value": text } }))
    }
}

fn publish(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics },
    })
}

fn lsp_diagnostic(diag: &brim_diagnostics::Diagnostic, range: Value) -> Value {
    let mut message = diag.message.clone();
    if let Some(label) = diag.primary.as_ref().filter(|l| !l.message.is_empty()) {
        message.push_str(&format!(": {}", label.message));
    }
    for note in &diag.notes {
        message.push_str(&format!("\nnote: {note}"));
    }
    for help in &diag.help {
        message.push_str(&format!("\nhelp: {help}"));
    }
    let severity = match diag.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
    };
    let mut value = json!({
        "range": range,
        "severity": severity,
        "source": "brim",
        "message": message,
    });
    if let Some(code) = &diag.code {
        value["code"] = json!(code.0);
    }
    value
}

fn symbol_json(symbol: &Symbol, text: &str, base: usize) -> Value {
    json!({
        "name": symbol.name,
        "kind": symbol.kind,
        "range": range(text, symbol.span.start - base, symbol.span.end - base),
        "selectionRange": range(text, symbol.name_span.start - base, symbol.name_span.end - base),
        "children": symbol.children.iter().map(|c| symbol_json(c, text, base)).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests;
