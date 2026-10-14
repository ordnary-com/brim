use std::fmt;

use crate::Span;

/// How serious a diagnostic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The program is rejected.
    Error,
    /// The program is accepted, but probably not what the author meant.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

/// A stable diagnostic code such as `E0200`.
///
/// Codes are documented in `docs/error-handling.md`. Once published a code is
/// never reused for a different problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Code(pub &'static str);

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A labelled region of source code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    /// Text shown next to the underline. May be empty.
    pub message: String,
}

/// A message from the compiler about a problem in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<Code>,
    pub message: String,
    /// The main location of the problem. `None` for whole-file problems such
    /// as a missing entry point.
    pub primary: Option<Label>,
    /// Related locations, rendered with `-` underlines.
    pub secondary: Vec<Label>,
    /// Extra context, rendered as `= note: ...`.
    pub notes: Vec<String>,
    /// Suggested fixes, rendered as `= help: ...`.
    pub help: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: None,
            message: message.into(),
            primary: None,
            secondary: Vec::new(),
            notes: Vec::new(),
            help: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(Code(code));
        self
    }

    /// Sets the primary location, optionally with a label message.
    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.primary = Some(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help.push(help.into());
        self
    }

    /// The primary span, if any.
    pub fn span(&self) -> Option<Span> {
        self.primary.as_ref().map(|l| l.span)
    }
}
