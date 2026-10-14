//! Run-time error messages for native executables.
//!
//! A native executable reports a run-time error exactly like `brim run`
//! does: a rendered diagnostic that quotes the failing source line, followed
//! by the Brim call stack. Everything except the call stack is known at
//! compile time, so the backend renders it here and embeds the text; the
//! runtime (`brim_rt_trap`) only prints it and appends the call-stack notes.

// Without the `llvm` feature nothing calls this module outside its tests.
#![cfg_attr(not(feature = "llvm"), allow(dead_code))]

use brim_diagnostics::{ColorChoice, Diagnostic, Renderer, SourceFile, Span};
use brim_runtime::RuntimeError;

/// The compile-time parts of a run-time error message.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TrapText {
    /// The diagnostic without notes.
    pub head: String,
    /// What separates `head` from the first note.
    pub sep: String,
    /// The text that starts a note line, before the note itself.
    pub note: String,
}

/// Renders the message for `error`, raised by the operation at `span`.
pub(crate) fn render(error: &RuntimeError, span: Option<Span>, source: &SourceFile) -> TrapText {
    const MARK: char = '\u{1}';
    let mut diag = Diagnostic::error(error.to_string()).with_code(error.code());
    if let Some(span) = span {
        diag = diag.with_primary(span, "");
    }
    // Errors are written to stderr; colors are not worth detecting there.
    let renderer = Renderer::new(ColorChoice::Never);
    let head = renderer.render(&diag, source);
    let with_note = renderer.render(&diag.with_note(MARK.to_string()), source);
    let mark = with_note
        .find(MARK)
        .expect("the renderer prints every note");
    let line_start = with_note[..mark].rfind('\n').map_or(0, |i| i + 1);
    debug_assert!(with_note.starts_with(&head));
    TrapText {
        sep: with_note[head.len()..line_start].to_string(),
        note: with_note[line_start..mark].to_string(),
        head,
    }
}

/// The message for an index outside a list, split around the part the
/// runtime fills in: `prefix`, "the index is I but the length is L",
/// `suffix`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct IndexTrapText {
    pub prefix: String,
    pub suffix: String,
    pub sep: String,
    pub note: String,
}

pub(crate) fn render_index(span: Option<Span>, source: &SourceFile) -> IndexTrapText {
    const HOLE: char = '\u{2}';
    // Render with placeholder numbers, then cut the sentence out.
    let error = RuntimeError::IndexOutOfBounds { index: 0, len: 0 };
    let sentence = "the index is 0 but the length is 0";
    let message = error.to_string().replace(sentence, &HOLE.to_string());
    let mut diag = Diagnostic::error(message).with_code(error.code());
    if let Some(span) = span {
        diag = diag.with_primary(span, "");
    }
    let renderer = Renderer::new(ColorChoice::Never);
    let head = renderer.render(&diag, source);
    let hole = head.find(HOLE).expect("the message contains the hole");
    let TrapText { sep, note, .. } = render(&error, span, source);
    IndexTrapText {
        prefix: head[..hole].to_string(),
        suffix: head[hole + HOLE.len_utf8()..].to_string(),
        sep,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_rendered_diagnostic() {
        let source = SourceFile::new("t.brim", "fn main() {\n    print(1 / 0)\n}\n");
        let text = render(
            &RuntimeError::DivisionByZero { op: "/" },
            Some(Span::new(22, 27)),
            &source,
        );
        assert_eq!(
            text.head,
            "error[R0002]: division by zero\n --> t.brim:2:11\n  |\n2 |     print(1 / 0)\n  |           ^^^^^\n"
        );
        assert_eq!(text.sep, "  |\n");
        assert_eq!(text.note, "  = note: ");
    }

    #[test]
    fn index_messages_have_a_hole() {
        let source = SourceFile::new("t.brim", "fn main() {\n    print(xs[9])\n}\n");
        let text = render_index(Some(Span::new(22, 27)), &source);
        assert_eq!(text.prefix, "error[R0005]: index out of bounds: ");
        assert!(text.suffix.starts_with("\n --> t.brim:2:11\n"));
        assert_eq!(text.note, "  = note: ");
    }

    #[test]
    fn without_a_span() {
        let source = SourceFile::new("t.brim", "");
        let text = render(&RuntimeError::StackOverflow { limit: 3 }, None, &source);
        assert_eq!(
            text.head,
            "error[R0003]: stack overflow: more than 3 nested calls\n --> t.brim\n"
        );
        assert_eq!(text.sep, "");
        assert_eq!(text.note, "  = note: ");
    }
}
