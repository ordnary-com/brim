//! HTML documentation for Brim packages (`brim doc`).
//!
//! Documentation comments are the `//` lines right above a declaration;
//! a module's documentation is the comment at the top of its file. Only
//! `pub` items are documented. In the text, `code` is code, a line indented
//! by two or more spaces is part of a code example, and an empty comment
//! line separates paragraphs.
//!
//! [`generate`] writes one page per module and an index with a search box.
//! Type names in signatures link to the types' documentation.

use std::collections::HashMap;
use std::fmt::Write as _;

use brim_ast::{FnDecl, Program};
use brim_diagnostics::Span;

/// A module to document.
#[derive(Debug)]
pub struct Module<'a> {
    /// The dotted module name: `notes`, `std.json`.
    pub name: String,
    pub program: &'a Program,
    /// The module's source text.
    pub text: &'a str,
    /// The offset of the file in the program's source map, which the
    /// spans of `program` include.
    pub base: usize,
}

/// The files of the documentation: `(file name, contents)`, the index
/// (`index.html`) first.
pub fn generate(modules: &[Module], title: &str) -> Vec<(String, String)> {
    let types = type_pages(modules);
    let mut files = vec![("index.html".to_string(), index_page(modules, title))];
    for module in modules {
        files.push((
            page_name(&module.name),
            module_page(module, modules, &types, title),
        ));
    }
    files.push(("style.css".to_string(), STYLE.to_string()));
    files
}

fn page_name(module: &str) -> String {
    format!("{module}.html")
}

/// Where each documented struct and enum is: `(module, name)` to the page.
fn type_pages(modules: &[Module]) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    for m in modules {
        let names = m
            .program
            .structs
            .iter()
            .filter(|s| s.public)
            .map(|s| &s.name.name)
            .chain(
                m.program
                    .enums
                    .iter()
                    .filter(|e| e.public)
                    .map(|e| &e.name.name),
            );
        for name in names {
            out.insert(
                (m.name.clone(), name.clone()),
                format!("{}#{name}", page_name(&m.name)),
            );
        }
    }
    out
}

// ---- doc comments ---------------------------------------------------------

/// The comment lines right above the line of `start` (an offset into
/// `text`), without `// `.
fn doc_above(text: &str, start: usize) -> Vec<String> {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let mut lines = Vec::new();
    let mut end = line_start;
    while end > 0 {
        let prev_start = text[..end - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = text[prev_start..end - 1].trim();
        match line.strip_prefix("//") {
            Some(rest) => lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string()),
            None => break,
        }
        end = prev_start;
    }
    lines.reverse();
    lines
}

/// The comment at the top of a file.
fn module_doc(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        match line.trim().strip_prefix("//") {
            Some(rest) => lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string()),
            None => break,
        }
    }
    lines
}

/// Doc comment lines as HTML: paragraphs, code examples and `code`.
fn render_doc(lines: &[String]) -> String {
    let mut out = String::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Vec<&str> = Vec::new();
    let flush_paragraph = |out: &mut String, paragraph: &mut Vec<&str>| {
        if !paragraph.is_empty() {
            let _ = write!(out, "<p>{}</p>", inline(&paragraph.join(" ")));
            paragraph.clear();
        }
    };
    let flush_code = |out: &mut String, code: &mut Vec<&str>| {
        if !code.is_empty() {
            let indent = code
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            let body: Vec<&str> = code
                .iter()
                .map(|l| if l.len() >= indent { &l[indent..] } else { "" })
                .collect();
            let _ = write!(
                out,
                "<pre><code>{}</code></pre>",
                escape(body.join("\n").trim_end())
            );
            code.clear();
        }
    };
    for line in lines {
        if line.starts_with("  ") {
            flush_paragraph(&mut out, &mut paragraph);
            code.push(line);
        } else if line.trim().is_empty() {
            if code.is_empty() {
                flush_paragraph(&mut out, &mut paragraph);
            } else {
                code.push("");
            }
        } else {
            flush_code(&mut out, &mut code);
            paragraph.push(line.trim());
        }
    }
    flush_paragraph(&mut out, &mut paragraph);
    flush_code(&mut out, &mut code);
    out
}

/// Text with `code` spans.
fn inline(text: &str) -> String {
    let mut out = String::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            let _ = write!(out, "<code>{}</code>", escape(part));
        } else {
            out.push_str(&escape(part));
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The first sentence of a doc comment, as plain text.
fn summary(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        if line.trim().is_empty() || line.starts_with("  ") {
            break;
        }
        text.push_str(line.trim());
        text.push(' ');
    }
    let text = text.trim();
    match text.find(". ") {
        Some(i) => text[..=i].to_string(),
        None => text.to_string(),
    }
}

// ---- pages ------------------------------------------------------------------

fn page(title: &str, heading: &str, nav: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{} - {}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n\
         <nav>{nav}</nav>\n<main>\n<h1>{}</h1>\n{body}</main>\n</body>\n</html>\n",
        escape(heading),
        escape(title),
        escape(heading)
    )
}

fn nav(modules: &[Module], title: &str, current: Option<&str>) -> String {
    let mut out = format!(
        "<a class=\"home\" href=\"index.html\">{}</a><ul>",
        escape(title)
    );
    for m in modules {
        let class = if current == Some(m.name.as_str()) {
            " class=\"current\""
        } else {
            ""
        };
        let _ = write!(
            out,
            "<li><a{class} href=\"{}\">{}</a></li>",
            page_name(&m.name),
            escape(&m.name)
        );
    }
    out.push_str("</ul>");
    out
}

fn index_page(modules: &[Module], title: &str) -> String {
    let mut body = String::from(
        "<input id=\"search\" type=\"search\" placeholder=\"Search\" autocomplete=\"off\">\n\
         <ul id=\"results\" class=\"items\"></ul>\n<section id=\"modules\"><h2>Modules</h2><dl>",
    );
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for m in modules {
        let _ = write!(
            body,
            "<dt><a href=\"{}\">{}</a></dt><dd>{}</dd>",
            page_name(&m.name),
            escape(&m.name),
            inline(&summary(&module_doc(m.text)))
        );
        for (name, _, doc) in items(m) {
            entries.push((
                format!("{}.{name}", m.name),
                format!("{}#{name}", page_name(&m.name)),
                summary(&doc),
            ));
        }
    }
    body.push_str("</dl></section>\n<script>\nconst items = [");
    for (name, href, text) in &entries {
        let _ = write!(
            body,
            "[{},{},{}],",
            js_string(name),
            js_string(href),
            js_string(text)
        );
    }
    body.push_str(SEARCH_SCRIPT);
    page(title, title, &nav(modules, title, None), &body)
}

fn js_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const SEARCH_SCRIPT: &str = "];
const input = document.getElementById('search');
const results = document.getElementById('results');
const modules = document.getElementById('modules');
input.addEventListener('input', () => {
  const q = input.value.trim().toLowerCase();
  results.replaceChildren();
  modules.hidden = q !== '';
  if (q === '') return;
  for (const [name, href, text] of items) {
    if (!name.toLowerCase().includes(q)) continue;
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = href;
    a.textContent = name;
    li.append(a, document.createTextNode(text ? ' ' + text : ''));
    results.append(li);
  }
});
</script>
";

/// The documented items of a module: `(name, kind, doc comment)`.
fn items(m: &Module) -> Vec<(String, &'static str, Vec<String>)> {
    let p = m.program;
    let doc = |span: Span| doc_above(m.text, span.start - m.base);
    let mut out = Vec::new();
    for s in p.structs.iter().filter(|s| s.public) {
        out.push((s.name.name.clone(), "struct", doc(s.span)));
    }
    for e in p.enums.iter().filter(|e| e.public) {
        out.push((e.name.name.clone(), "enum", doc(e.span)));
    }
    for t in p.traits.iter().filter(|t| t.public) {
        out.push((t.name.name.clone(), "trait", doc(t.span)));
    }
    for c in p.consts.iter().filter(|c| c.public) {
        out.push((c.name.name.clone(), "const", doc(c.span)));
    }
    for f in p.functions.iter().filter(|f| f.public) {
        out.push((f.name.name.clone(), "fn", doc(f.span)));
    }
    out
}

fn module_page(
    m: &Module,
    modules: &[Module],
    types: &HashMap<(String, String), String>,
    title: &str,
) -> String {
    let p = m.program;
    let linker = Linker { module: m, types };
    let text = |span: Span| &m.text[span.start - m.base..span.end - m.base];
    let doc = |span: Span| render_doc(&doc_above(m.text, span.start - m.base));
    let mut body = render_doc(&module_doc(m.text));

    let structs: Vec<_> = p.structs.iter().filter(|s| s.public).collect();
    let enums: Vec<_> = p.enums.iter().filter(|e| e.public).collect();
    let traits: Vec<_> = p.traits.iter().filter(|t| t.public).collect();
    let consts: Vec<_> = p.consts.iter().filter(|c| c.public).collect();
    let functions: Vec<_> = p.functions.iter().filter(|f| f.public).collect();

    if !structs.is_empty() || !enums.is_empty() {
        body.push_str("<h2>Types</h2>");
    }
    for s in &structs {
        let params = type_params(&s.type_params);
        let mut sig = format!("pub struct {}{params} {{", s.name.name);
        let mut fields = String::new();
        let mut private = false;
        for f in &s.fields {
            if !f.public {
                private = true;
                continue;
            }
            let _ = write!(sig, "\n    pub {}: {},", f.name.name, text(f.ty.span()));
            let field_doc = doc(f.span);
            if !field_doc.is_empty() {
                let _ = write!(
                    fields,
                    "<dt><code>{}</code></dt><dd>{field_doc}</dd>",
                    escape(&f.name.name)
                );
            }
        }
        if private {
            sig.push_str("\n    // and private fields");
        }
        sig.push_str(if s.fields.is_empty() { "}" } else { "\n}" });
        let _ = write!(
            body,
            "<section class=\"item\" id=\"{0}\"><h3><a href=\"#{0}\">struct {0}</a></h3><pre class=\"sig\"><code>{1}</code></pre>{2}",
            escape(&s.name.name),
            linker.link(&sig),
            doc(s.span)
        );
        if !fields.is_empty() {
            let _ = write!(body, "<dl class=\"fields\">{fields}</dl>");
        }
        body.push_str(&methods(m, &linker, &s.name.name));
        body.push_str("</section>");
    }
    for e in &enums {
        let params = type_params(&e.type_params);
        let mut sig = format!("pub enum {}{params} {{", e.name.name);
        let mut variants = String::new();
        for v in &e.variants {
            let _ = write!(sig, "\n    {},", text(v.span).trim_end_matches(','));
            let variant_doc = doc(v.span);
            if !variant_doc.is_empty() {
                let _ = write!(
                    variants,
                    "<dt><code>{}</code></dt><dd>{variant_doc}</dd>",
                    escape(&v.name.name)
                );
            }
        }
        sig.push_str("\n}");
        let _ = write!(
            body,
            "<section class=\"item\" id=\"{0}\"><h3><a href=\"#{0}\">enum {0}</a></h3><pre class=\"sig\"><code>{1}</code></pre>{2}",
            escape(&e.name.name),
            linker.link(&sig),
            doc(e.span)
        );
        if !variants.is_empty() {
            let _ = write!(body, "<dl class=\"fields\">{variants}</dl>");
        }
        body.push_str(&methods(m, &linker, &e.name.name));
        body.push_str("</section>");
    }
    if !traits.is_empty() {
        body.push_str("<h2>Traits</h2>");
    }
    for t in &traits {
        let _ = write!(
            body,
            "<section class=\"item\" id=\"{0}\"><h3><a href=\"#{0}\">trait {0}</a></h3><pre class=\"sig\"><code>{1}</code></pre>{2}</section>",
            escape(&t.name.name),
            linker.link(&format!("pub {}", text(t.span))),
            doc(t.span)
        );
    }
    if !consts.is_empty() {
        body.push_str("<h2>Constants</h2>");
    }
    for c in &consts {
        let _ = write!(
            body,
            "<section class=\"item\" id=\"{0}\"><h3><a href=\"#{0}\">{0}</a></h3><pre class=\"sig\"><code>{1}</code></pre>{2}</section>",
            escape(&c.name.name),
            linker.link(&format!("pub {}", text(c.span))),
            doc(c.span)
        );
    }
    if !functions.is_empty() {
        body.push_str("<h2>Functions</h2>");
    }
    for f in &functions {
        let _ = write!(
            body,
            "<section class=\"item\" id=\"{0}\"><h3><a href=\"#{0}\">fn {0}</a></h3><pre class=\"sig\"><code>{1}</code></pre>{2}</section>",
            escape(&f.name.name),
            linker.link(&header(m, f)),
            doc(f.span)
        );
    }
    page(title, &m.name, &nav(modules, title, Some(&m.name)), &body)
}

fn type_params(params: &[brim_ast::Ident]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        format!("[{}]", names.join(", "))
    }
}

/// `pub fn name(params) -> ret`.
fn header(m: &Module, f: &FnDecl) -> String {
    // Up to the body: `signature_span` ends at the name when there are no
    // parameters.
    let text = m.text[f.span.start - m.base..f.body.span.start - m.base].trim_end();
    if text.starts_with("pub ") {
        text.to_string()
    } else {
        format!("pub {text}")
    }
}

/// The `pub` methods of a type, from the `impl` blocks of its module.
fn methods(m: &Module, linker: &Linker, type_name: &str) -> String {
    let mut out = String::new();
    for i in m
        .program
        .impls
        .iter()
        .filter(|i| i.type_name.name == type_name)
    {
        for f in i
            .methods
            .iter()
            .filter(|f| f.public || i.trait_name.is_some())
        {
            let id = format!("{type_name}.{}", f.name.name);
            let _ = write!(
                out,
                "<div class=\"method\" id=\"{0}\"><pre class=\"sig\"><code>{1}</code></pre>{2}</div>",
                escape(&id),
                linker.link(&header(m, f)),
                render_doc(&doc_above(m.text, f.span.start - m.base))
            );
        }
    }
    if out.is_empty() {
        out
    } else {
        format!("<h4>Methods</h4>{out}")
    }
}

/// Turns type names in signatures into links.
struct Linker<'a> {
    module: &'a Module<'a>,
    types: &'a HashMap<(String, String), String>,
}

impl Linker<'_> {
    /// `code`, escaped, with each documented type name a link.
    fn link(&self, code: &str) -> String {
        let mut out = String::new();
        let bytes = code.as_bytes();
        let mut i = 0;
        while i < code.len() {
            let c = bytes[i];
            if c.is_ascii_alphabetic() || c == b'_' {
                let start = i;
                while i < code.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &code[start..i];
                // `alias.Name`
                if i + 1 < code.len()
                    && bytes[i] == b'.'
                    && bytes[i + 1].is_ascii_uppercase()
                    && let Some(module) = self.alias(word)
                {
                    let name_start = i + 1;
                    let mut end = name_start;
                    while end < code.len()
                        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    let name = &code[name_start..end];
                    if let Some(href) = self.types.get(&(module, name.to_string())) {
                        let _ = write!(
                            out,
                            "<a href=\"{href}\">{}.{}</a>",
                            escape(word),
                            escape(name)
                        );
                        i = end;
                        continue;
                    }
                }
                match self
                    .types
                    .get(&(self.module.name.clone(), word.to_string()))
                {
                    Some(href) => {
                        let _ = write!(out, "<a href=\"{href}\">{}</a>", escape(word));
                    }
                    None => out.push_str(&escape(word)),
                }
                continue;
            }
            let ch = code[i..].chars().next().expect("a character");
            out.push_str(&escape(&ch.to_string()));
            i += ch.len_utf8();
        }
        out
    }

    /// The module an import of this module calls `alias`.
    fn alias(&self, alias: &str) -> Option<String> {
        self.module.program.imports.iter().find_map(|import| {
            if import.items.is_some() {
                return None;
            }
            let name = import.alias.as_ref().or(import.path.last())?;
            (name.name == alias).then(|| {
                import
                    .path
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            })
        })
    }
}

const STYLE: &str = r#":root {
  --bg: #ffffff; --fg: #1b1d21; --muted: #5d6470; --line: #e3e6eb;
  --code-bg: #f4f5f7; --accent: #b0480f; --nav-bg: #fafafa;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #15171a; --fg: #e6e8eb; --muted: #9aa1ab; --line: #2a2e34;
    --code-bg: #1e2125; --accent: #f08a4b; --nav-bg: #191b1f;
  }
}
* { box-sizing: border-box; }
body { margin: 0; display: flex; min-height: 100vh; background: var(--bg); color: var(--fg);
  font: 16px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif; }
nav { width: 15rem; flex: none; padding: 1.25rem 1rem; border-right: 1px solid var(--line);
  background: var(--nav-bg); position: sticky; top: 0; height: 100vh; overflow-y: auto; }
nav .home { font-weight: 700; color: var(--fg); text-decoration: none; }
nav ul { list-style: none; padding: 0; margin: 1rem 0 0; }
nav li a { display: block; padding: .15rem 0; color: var(--muted); text-decoration: none;
  font-family: ui-monospace, Menlo, Consolas, monospace; font-size: .875rem; }
nav li a.current, nav li a:hover { color: var(--accent); }
main { flex: 1; min-width: 0; max-width: 56rem; padding: 2rem 2.5rem 4rem; }
h1 { margin-top: 0; font-family: ui-monospace, Menlo, Consolas, monospace; }
h2 { margin-top: 2.5rem; border-bottom: 1px solid var(--line); padding-bottom: .3rem; }
h3 { margin: 0 0 .5rem; font-family: ui-monospace, Menlo, Consolas, monospace; font-size: 1.05rem; }
h3 a { color: var(--fg); text-decoration: none; }
h4 { margin: 1.25rem 0 .5rem; color: var(--muted); font-size: .9rem; text-transform: uppercase; letter-spacing: .04em; }
a { color: var(--accent); }
code, pre { font-family: ui-monospace, Menlo, Consolas, monospace; font-size: .875rem; }
p code, dd code, li code { background: var(--code-bg); padding: .1em .3em; border-radius: 4px; }
pre { background: var(--code-bg); padding: .75rem 1rem; border-radius: 6px; overflow-x: auto; }
.item { padding: 1.25rem 0; border-bottom: 1px solid var(--line); }
.method { margin-left: 1rem; }
dl.fields dt { margin-top: .5rem; }
dl.fields dd { margin-left: 1rem; color: var(--muted); }
dl.fields dd p { margin: 0; }
#search { width: 100%; padding: .6rem .8rem; font-size: 1rem; border: 1px solid var(--line);
  border-radius: 6px; background: var(--bg); color: var(--fg); }
ul.items { list-style: none; padding: 0; }
ul.items li { padding: .3rem 0; color: var(--muted); }
ul.items a { font-family: ui-monospace, Menlo, Consolas, monospace; margin-right: .5rem; }
section#modules dt { margin-top: .75rem; font-family: ui-monospace, Menlo, Consolas, monospace; }
section#modules dd { margin-left: 0; color: var(--muted); }
@media (max-width: 760px) {
  body { display: block; }
  nav { position: static; width: auto; height: auto; border-right: 0; border-bottom: 1px solid var(--line); }
  main { padding: 1.25rem 1rem 3rem; }
}
"#;

#[cfg(test)]
mod tests;
