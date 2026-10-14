//! The wire format of the Language Server Protocol: messages with a
//! `Content-Length` header, positions in UTF-16 code units, and file URIs.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Reads one message. `None` at the end of the input.
pub(crate) fn read_message(input: &mut dyn BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message without Content-Length",
        ));
    };
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub(crate) fn write_message(output: &mut dyn Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

/// An LSP position (zero-based line, UTF-16 column) for a byte offset.
pub(crate) fn position(text: &str, offset: usize) -> Value {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    serde_json::json!({ "line": line, "character": character })
}

pub(crate) fn range(text: &str, start: usize, end: usize) -> Value {
    serde_json::json!({ "start": position(text, start), "end": position(text, end) })
}

/// The byte offset of an LSP position; positions past the end of a line
/// or the text are clamped.
pub(crate) fn offset(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap_or(0) as usize;
    let character = position["character"].as_u64().unwrap_or(0) as usize;
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let line_text = &text[start..];
    let line_text = &line_text[..line_text.find('\n').unwrap_or(line_text.len())];
    let mut units = 0;
    for (i, c) in line_text.char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    start + line_text.len()
}

/// The path of a `file:` URI.
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::new();
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            bytes.push(b);
            i += 3;
            continue;
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let mut path = String::from_utf8(bytes).ok()?;
    // `file:///c:/dir` on Windows.
    if path.len() > 2 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

/// The `file:` URI of a path.
pub(crate) fn path_to_uri(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}
