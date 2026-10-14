//! Source files, spans and compiler diagnostics for Brim.
//!
//! This crate is the bottom of the compiler's dependency graph. Every phase
//! (lexer, parser, semantic analysis, lowering, execution) reports problems as
//! [`Diagnostic`] values that point into a [`SourceFile`] through [`Span`]s.
//!
//! # Positions
//!
//! A [`Span`] stores half-open **byte offsets** into the UTF-8 source text.
//! Line and column numbers are not stored in spans; they are computed on
//! demand by [`SourceFile::location`] from a precomputed line table. This keeps
//! spans small (two integers) and cheap to copy, while still giving every
//! diagnostic exact line/column information.
//!
//! Displayed positions are 1-based. Columns count Unicode scalar values
//! (`char`s), not bytes, so `"é"` advances the column by one even though it is
//! two bytes long. Columns do not account for grapheme clusters or terminal
//! display width (e.g. East Asian wide characters); see `docs/architecture.md`.

mod diagnostic;
mod render;
mod source;

pub use diagnostic::{Code, Diagnostic, Label, Severity};
pub use render::{ColorChoice, Renderer};
pub use source::{Location, SourceFile, Span};

/// Returns `true` if any diagnostic in `diags` is an error.
pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}
