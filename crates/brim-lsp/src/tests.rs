use super::*;
use std::fs;

fn frame(message: &Value) -> String {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

/// Runs a session and returns the messages the server sent.
fn session(messages: &[Value]) -> (i32, Vec<Value>) {
    let input: String = messages.iter().map(frame).collect();
    let mut reader = io::Cursor::new(input.into_bytes());
    let mut output = Vec::new();
    let code = run(&mut reader, &mut output).unwrap();
    let mut replies = Vec::new();
    let mut cursor = io::Cursor::new(output);
    while let Some(m) = protocol::read_message(&mut cursor).unwrap() {
        replies.push(m);
    }
    (code, replies)
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn reply(replies: &[Value], id: i64) -> &Value {
    &replies
        .iter()
        .find(|r| r["id"] == json!(id))
        .unwrap_or_else(|| panic!("no reply {id} in {replies:#?}"))["result"]
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("brim-lsp-tests-{}", std::process::id()))
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    // Canonical, so paths compare equal to those derived from URIs, but
    // without the `\\?\` prefix Windows adds.
    let canonical = dir.canonicalize().unwrap().display().to_string();
    PathBuf::from(canonical.strip_prefix(r"\\?\").unwrap_or(&canonical))
}

const MAIN: &str = "import shapes\n\nfn main() {\n    let side: i64 = 3\n    let sq: shapes.Square = shapes.square(side)\n    print(sq.side + area(sq))\n}\n\nfn area(s: shapes.Square) -> i64 {\n    return s.side * s.side\n}\n";
const SHAPES: &str = "pub struct Square {\n    side: i64,\n}\n\npub fn square(side: i64) -> Square {\n    return Square { side: side }\n}\n";

fn project(name: &str) -> (PathBuf, String) {
    let dir = scratch(name);
    fs::write(
        dir.join("brim.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    )
    .unwrap();
    fs::write(dir.join("src/main.brim"), MAIN).unwrap();
    fs::write(dir.join("src/shapes.brim"), SHAPES).unwrap();
    let uri = path_to_uri(&dir.join("src/main.brim"));
    (dir, uri)
}

fn open(uri: &str, text: &str) -> Value {
    notification(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "brim", "version": 1, "text": text } }),
    )
}

fn at(uri: &str, line: u32, character: u32) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } })
}

#[test]
fn lifecycle() {
    let (code, replies) = session(&[
        request(1, "initialize", json!({ "capabilities": {} })),
        notification("initialized", json!({})),
        request(2, "frobnicate", json!({})),
        request(3, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ]);
    assert_eq!(code, 0);
    assert_eq!(
        reply(&replies, 1)["capabilities"]["hoverProvider"],
        json!(true)
    );
    assert_eq!(replies[1]["error"]["code"], json!(-32601));
    assert_eq!(reply(&replies, 3), &Value::Null);

    let (code, _) = session(&[notification("exit", Value::Null)]);
    assert_eq!(code, 1);
}

#[test]
fn diagnostics_follow_edits() {
    let (dir, uri) = project("diagnostics");
    let broken = MAIN.replace("area(sq))", "area(true))");
    let (_, replies) = session(&[
        open(&uri, MAIN),
        notification(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": broken }] }),
        ),
    ]);
    let for_main: Vec<&Value> = replies
        .iter()
        .filter(|r| r["params"]["uri"] == json!(uri))
        .collect();
    assert_eq!(for_main.len(), 2, "{replies:#?}");
    assert_eq!(for_main[0]["params"]["diagnostics"], json!([]));
    let diag = &for_main[1]["params"]["diagnostics"][0];
    assert_eq!(diag["code"], json!("E0206"), "{diag:#?}");
    assert_eq!(diag["severity"], json!(1));
    assert_eq!(
        diag["range"]["start"],
        json!({ "line": 5, "character": 25 })
    );
    // The other module of the program gets (empty) diagnostics too.
    let shapes = path_to_uri(&dir.join("src/shapes.brim"));
    assert!(replies.iter().any(|r| r["params"]["uri"] == json!(shapes)));
}

#[test]
fn definition_and_hover() {
    let (dir, uri) = project("definition");
    let shapes = path_to_uri(&dir.join("src/shapes.brim"));
    let (_, replies) = session(&[
        open(&uri, MAIN),
        // `side` in `shapes.square(side)`: the variable.
        request(1, "textDocument/definition", at(&uri, 4, 46)),
        request(2, "textDocument/hover", at(&uri, 4, 46)),
        // `square` in `shapes.square`: the function in the other module.
        request(3, "textDocument/definition", at(&uri, 4, 38)),
        request(4, "textDocument/hover", at(&uri, 4, 38)),
        // `area`: a function of this module.
        request(5, "textDocument/hover", at(&uri, 5, 22)),
        // `side` in `s.side`: the field.
        request(6, "textDocument/hover", at(&uri, 9, 14)),
        // `s`: the parameter.
        request(7, "textDocument/hover", at(&uri, 9, 11)),
        // Whitespace: nothing.
        request(8, "textDocument/hover", at(&uri, 2, 11)),
    ]);
    assert_eq!(
        reply(&replies, 1),
        &json!([{ "uri": uri, "range": { "start": { "line": 3, "character": 8 }, "end": { "line": 3, "character": 12 } } }])
    );
    assert_eq!(
        reply(&replies, 2)["contents"]["value"],
        json!("```brim\nlet side: i64\n```")
    );
    assert_eq!(reply(&replies, 3)[0]["uri"], json!(shapes));
    assert_eq!(
        reply(&replies, 3)[0]["range"]["start"],
        json!({ "line": 4, "character": 7 })
    );
    assert_eq!(
        reply(&replies, 4)["contents"]["value"],
        json!("```brim\npub fn square(side: i64) -> Square\n```")
    );
    assert_eq!(
        reply(&replies, 5)["contents"]["value"],
        json!("```brim\nfn area(s: shapes.Square) -> i64\n```")
    );
    assert_eq!(
        reply(&replies, 6)["contents"]["value"],
        json!("```brim\nSquare.side: i64\n```")
    );
    assert_eq!(
        reply(&replies, 7)["contents"]["value"],
        json!("```brim\ns: shapes.Square\n```")
    );
    assert_eq!(reply(&replies, 8), &Value::Null);
}

#[test]
fn symbols_and_formatting() {
    let (_, uri) = project("symbols");
    let messy = "fn main(){\nprint( 1 )\n}\n";
    let (_, replies) = session(&[
        open(&uri, MAIN),
        request(
            1,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        ),
        request(
            2,
            "textDocument/formatting",
            json!({ "textDocument": { "uri": uri }, "options": {} }),
        ),
        notification(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri }, "contentChanges": [{ "text": messy }] }),
        ),
        request(
            3,
            "textDocument/formatting",
            json!({ "textDocument": { "uri": uri }, "options": {} }),
        ),
    ]);
    let names: Vec<&str> = reply(&replies, 1)
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main", "area"]);
    assert_eq!(reply(&replies, 2), &json!([]));
    assert_eq!(
        reply(&replies, 3)[0]["newText"],
        json!("fn main() {\n    print(1)\n}\n")
    );
}

#[test]
fn positions_and_uris() {
    let text = "a\n€x𝄞y\n";
    assert_eq!(
        protocol::position(text, 2),
        json!({ "line": 1, "character": 0 })
    );
    // `€` is one UTF-16 unit, `𝄞` two.
    assert_eq!(
        protocol::position(text, 6),
        json!({ "line": 1, "character": 2 })
    );
    assert_eq!(
        protocol::position(text, 10),
        json!({ "line": 1, "character": 4 })
    );
    assert_eq!(offset(text, &json!({ "line": 1, "character": 4 })), 10);
    assert_eq!(offset(text, &json!({ "line": 1, "character": 99 })), 11);
    assert_eq!(
        offset(text, &json!({ "line": 9, "character": 0 })),
        text.len()
    );

    let path = PathBuf::from("/tmp/my project/main.brim");
    let uri = path_to_uri(&path);
    assert_eq!(uri, "file:///tmp/my%20project/main.brim");
    assert_eq!(uri_to_path(&uri), Some(path));
    assert_eq!(
        uri_to_path("file:///c%3A/src/main.brim"),
        Some(PathBuf::from("c:/src/main.brim"))
    );
}

/// The editor grammars know every keyword and builtin.
#[test]
fn grammars_are_complete() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editors");
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    let textmate = read("vscode/syntaxes/brim.tmLanguage.json");
    let grammar: Value = serde_json::from_str(&textmate).unwrap();
    assert_eq!(grammar["scopeName"], json!("source.brim"));
    let keywords: Vec<&str> = brim_lexer::KEYWORDS.iter().map(|(word, _)| *word).collect();
    let reserved = brim_lexer::RESERVED;
    let builtins: Vec<&str> = brim_stdlib_names().collect();

    // Grammars that list every word themselves.
    for file in [
        "vscode/syntaxes/brim.tmLanguage.json",
        "vim/syntax/brim.vim",
        "sublime/Brim.sublime-syntax",
        "emacs/brim-mode.el",
        "kate/brim.xml",
        "nano/brim.nanorc",
        "micro/brim.yaml",
        "kakoune/brim.kak",
        "notepad++/brim.xml",
    ] {
        let text = read(file);
        for word in keywords.iter().chain(reserved).chain(&builtins) {
            assert!(contains_word(&text, word), "{file} misses `{word}`");
        }
    }
    // The tree-sitter grammar has the keywords; its queries the builtins.
    let tree_sitter = read("tree-sitter-brim/grammar.js");
    for word in &keywords {
        assert!(
            tree_sitter.contains(&format!("'{word}'")),
            "tree-sitter-brim/grammar.js misses `{word}`"
        );
    }
    let queries = read("tree-sitter-brim/queries/highlights.scm");
    for word in &builtins {
        assert!(
            queries.contains(&format!("\"{word}\"")),
            "highlights.scm misses `{word}`"
        );
    }
}

/// Whether `word` occurs in `text` as a whole word.
fn contains_word(text: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word)
        .any(|(i, _)| !text[..i].ends_with(is_word) && !text[i + word.len()..].starts_with(is_word))
}

fn brim_stdlib_names() -> impl Iterator<Item = &'static str> {
    [
        "print",
        "eprint",
        "to_string",
        "to_float",
        "to_int",
        "to_i32",
        "to_u8",
        "to_u64",
        "string_from_bytes",
        "read_line",
        "read_file",
        "write_file",
        "args",
        "env",
        "exit",
        "assert",
        "assert_eq",
    ]
    .into_iter()
}
