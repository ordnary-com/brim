use std::fmt::Write as _;

use crate::{Diagnostic, Label, Severity, SourceFile};

/// Number of columns a tab character occupies in rendered source lines.
const TAB_WIDTH: usize = 4;

/// Whether rendered diagnostics contain ANSI color escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Never,
    Always,
}

/// Renders diagnostics in a format modelled on modern compilers:
///
/// ```text
/// error[E0200]: undefined variable `username`
///  --> examples/hello.brim:4:11
///   |
/// 4 |     print(username)
///   |           ^^^^^^^^ not found in this scope
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct Renderer {
    color: ColorChoice,
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.on && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

const BOLD: &str = "1";
const RED: &str = "1;31";
const YELLOW: &str = "1;33";
const BLUE: &str = "1;34";

impl Renderer {
    pub fn new(color: ColorChoice) -> Self {
        Renderer { color }
    }

    /// Renders one diagnostic, including a trailing newline.
    pub fn render(&self, diag: &Diagnostic, file: &SourceFile) -> String {
        let style = Style {
            on: self.color == ColorChoice::Always,
        };
        let severity_color = match diag.severity {
            Severity::Error => RED,
            Severity::Warning => YELLOW,
        };
        let mut out = String::new();

        let mut header = diag.severity.to_string();
        if let Some(code) = diag.code {
            let _ = write!(header, "[{code}]");
        }
        let _ = writeln!(
            out,
            "{}{}",
            style.paint(severity_color, &header),
            style.paint(BOLD, &format!(": {}", diag.message))
        );

        // Collect labels in source order; the primary one is marked with '^'.
        let mut labels: Vec<(&Label, bool)> = Vec::new();
        if let Some(primary) = &diag.primary {
            labels.push((primary, true));
        }
        labels.extend(diag.secondary.iter().map(|l| (l, false)));

        let max_line = labels
            .iter()
            .map(|(l, _)| file.location(l.span.start).line)
            .max()
            .unwrap_or(1);
        let width = max_line.to_string().len();
        let pad = " ".repeat(width);
        let bar = style.paint(BLUE, "|");

        let anchor = diag.primary.as_ref().or(diag.secondary.first());
        match anchor {
            Some(label) => {
                let loc = file.location(label.span.start);
                let _ = writeln!(
                    out,
                    "{pad}{} {}:{}",
                    style.paint(BLUE, "-->"),
                    file.name(),
                    loc
                );
            }
            None => {
                let _ = writeln!(out, "{pad}{} {}", style.paint(BLUE, "-->"), file.name());
            }
        }

        if !labels.is_empty() {
            let _ = writeln!(out, "{pad} {bar}");
            labels.sort_by_key(|(l, primary)| (l.span.start, !primary));
            let mut last_line = None;
            for (label, primary) in &labels {
                let line = file.location(label.span.start).line;
                let text = file.line_text(line);
                if last_line != Some(line) {
                    if let Some(prev) = last_line
                        && line > prev + 1
                    {
                        let _ = writeln!(out, "{}", style.paint(BLUE, "..."));
                    }
                    let _ = writeln!(
                        out,
                        "{} {bar} {}",
                        style.paint(BLUE, &format!("{line:>width$}")),
                        expand_tabs(text)
                    );
                    last_line = Some(line);
                }
                let line_start = file.line_start(line);
                let before = &text[..(label.span.start - line_start).min(text.len())];
                let end_in_line = (label.span.end - line_start).min(text.len());
                let covered = &text[before.len()..end_in_line.max(before.len())];
                let indent = display_width(before);
                let len = display_width(covered).max(1);
                let (mark, color) = if *primary {
                    ('^', severity_color)
                } else {
                    ('-', BLUE)
                };
                let marks: String = std::iter::repeat_n(mark, len).collect();
                let mut underline = format!("{}{}", " ".repeat(indent), style.paint(color, &marks));
                if !label.message.is_empty() {
                    let _ = write!(underline, " {}", style.paint(color, &label.message));
                }
                let _ = writeln!(out, "{pad} {bar} {underline}");
            }
        }

        if !diag.notes.is_empty() || !diag.help.is_empty() {
            if !labels.is_empty() {
                let _ = writeln!(out, "{pad} {bar}");
            }
            let eq = style.paint(BLUE, "=");
            for note in &diag.notes {
                let _ = writeln!(out, "{pad} {eq} {}: {note}", style.paint(BOLD, "note"));
            }
            for help in &diag.help {
                let _ = writeln!(out, "{pad} {eq} {}: {help}", style.paint(BOLD, "help"));
            }
        }
        out
    }

    /// Renders several diagnostics separated by blank lines.
    pub fn render_all(&self, diags: &[Diagnostic], file: &SourceFile) -> String {
        diags
            .iter()
            .map(|d| self.render(d, file))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

fn expand_tabs(s: &str) -> String {
    s.replace('\t', &" ".repeat(TAB_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diagnostic, Span};

    fn render(diag: &Diagnostic, src: &str) -> String {
        Renderer::default().render(diag, &SourceFile::new("examples/hello.brim", src))
    }

    #[test]
    fn renders_primary_label() {
        let src = "fn main() {\n\n\n    print(username)\n}\n";
        let start = src.find("username").unwrap();
        let diag = Diagnostic::error("undefined variable `username`")
            .with_code("E0200")
            .with_primary(Span::new(start, start + 8), "not found in this scope");
        assert_eq!(
            render(&diag, src),
            "error[E0200]: undefined variable `username`\n \
             --> examples/hello.brim:4:11\n  \
             |\n\
             4 |     print(username)\n  \
             |           ^^^^^^^^ not found in this scope\n"
        );
    }

    #[test]
    fn renders_notes_help_and_secondary() {
        let src = "fn f() {}\nfn f() {}\n";
        let diag = Diagnostic::error("duplicate function `f`")
            .with_primary(Span::new(13, 14), "redefined here")
            .with_secondary(Span::new(3, 4), "first defined here")
            .with_note("functions share one namespace")
            .with_help("rename one of them");
        assert_eq!(
            render(&diag, src),
            "error: duplicate function `f`\n \
             --> examples/hello.brim:2:4\n  \
             |\n\
             1 | fn f() {}\n  \
             |    - first defined here\n\
             2 | fn f() {}\n  \
             |    ^ redefined here\n  \
             |\n  \
             = note: functions share one namespace\n  \
             = help: rename one of them\n"
        );
    }

    #[test]
    fn renders_without_location() {
        let diag = Diagnostic::error("no `main` function").with_help("add `fn main() {}`");
        assert_eq!(
            render(&diag, ""),
            "error: no `main` function\n --> examples/hello.brim\n  = help: add `fn main() {}`\n"
        );
    }

    #[test]
    fn tabs_are_expanded_consistently() {
        let src = "\tx";
        let diag = Diagnostic::warning("w").with_primary(Span::new(1, 2), "");
        assert_eq!(
            render(&diag, src),
            "warning: w\n --> examples/hello.brim:1:2\n  |\n1 |     x\n  |     ^\n"
        );
    }

    #[test]
    fn empty_span_at_end_of_line_gets_one_caret() {
        let src = "abc\n";
        let diag = Diagnostic::error("e").with_primary(Span::point(3), "here");
        assert!(render(&diag, src).contains("  |    ^ here\n"));
    }

    #[test]
    fn color_output_contains_escapes() {
        let diag = Diagnostic::error("e");
        let out = Renderer::new(ColorChoice::Always).render(&diag, &SourceFile::new("f", ""));
        assert!(out.contains("\x1b[1;31merror\x1b[0m"));
    }
}
