use super::*;

const SOURCE: &str = "// Notes, kept in memory.
//
//   let n = notes.new_note(\"milk\")
import shapes

// A note.
pub struct Note {
    // Its number.
    pub id: i64,
    pub text: string,
    secret: string,
}

pub enum Kind {
    // Something to do.
    Todo,
    Done(i64),
}

impl Note {
    // Whether it is empty.
    pub fn is_empty(self) -> bool {
        return self.text.is_empty()
    }

    fn hidden(self) {}
}

// The largest `id` a note can have.
pub const MAX: i64 = 100

// Makes a note
// with `text`.
pub fn new_note(text: string) -> Note {
    return Note { id: 1, text: text, secret: \"\" }
}

pub fn area(s: shapes.Square) -> i64 {
    return 1
}

fn private() {}
";

fn docs() -> Vec<(String, String)> {
    let (program, diags) = brim_parser::parse(SOURCE);
    assert!(diags.is_empty(), "{diags:?}");
    let (shapes, _) = brim_parser::parse("pub struct Square {\n    pub side: i64,\n}\n");
    let modules = [
        Module {
            name: "notes".into(),
            program: &program,
            text: SOURCE,
            base: 0,
        },
        Module {
            name: "shapes".into(),
            program: &shapes,
            text: "pub struct Square {\n    pub side: i64,\n}\n",
            base: 0,
        },
    ];
    generate(&modules, "demo")
}

fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    &files.iter().find(|(n, _)| n == name).unwrap().1
}

#[test]
fn module_pages() {
    let files = docs();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["index.html", "notes.html", "shapes.html", "style.css"]
    );
    let notes = file(&files, "notes.html");
    // The module comment, with its example as code.
    assert!(notes.contains("<p>Notes, kept in memory.</p>"), "{notes}");
    assert!(notes.contains("<pre><code>let n = notes.new_note(&quot;milk&quot;)</code></pre>"));
    // Public items only; private fields are mentioned, not shown.
    assert!(notes.contains("id=\"Note\""));
    assert!(notes.contains("// and private fields"));
    assert!(!notes.contains("secret"));
    assert!(!notes.contains("private()"));
    assert!(!notes.contains("hidden"));
    // Comments of fields, variants, methods, constants and functions.
    assert!(notes.contains("<dt><code>id</code></dt><dd><p>Its number.</p></dd>"));
    assert!(notes.contains("<dt><code>Todo</code></dt><dd><p>Something to do.</p></dd>"));
    assert!(notes.contains("<p>Whether it is empty.</p>"));
    assert!(notes.contains("pub fn is_empty(self) -&gt; bool"));
    assert!(notes.contains("<p>The largest <code>id</code> a note can have.</p>"));
    assert!(notes.contains("<p>Makes a note with <code>text</code>.</p>"));
    // Type names link to their documentation, also in other modules.
    assert!(
        notes.contains("pub fn new_note(text: string) -&gt; <a href=\"notes.html#Note\">Note</a>")
    );
    assert!(notes.contains("<a href=\"shapes.html#Square\">shapes.Square</a>"));
}

#[test]
fn index_page() {
    let files = docs();
    let index = file(&files, "index.html");
    assert!(index.contains("<a href=\"notes.html\">notes</a></dt><dd>Notes, kept in memory.</dd>"));
    assert!(
        index
            .contains("[\"notes.new_note\",\"notes.html#new_note\",\"Makes a note with `text`.\"]")
    );
    assert!(index.contains("<title>demo - demo</title>"));
}
