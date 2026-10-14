//! The Brim formatter, used by `brim fmt`.
//!
//! The formatter works on tokens, not on the syntax tree, so it keeps what a
//! tree would lose: comments, blank lines and where the author broke lines.
//! It normalises everything else:
//!
//! - indentation: four spaces per open `{`, `(` or `[` whose contents start
//!   on a new line, plus one level for a line that continues an expression
//!   or the contents of a bracket opened on an earlier line;
//! - spaces between tokens on a line (`a + b`, `f(x, y)`, `-x`, `xs[0]`,
//!   `Point { x: 1 }`, `import a.{b, c}`);
//! - blank lines: at most one in a row, none at the start of a block or
//!   before its end, and none at the start or end of the file;
//! - comments keep their text, with trailing whitespace removed.
//!
//! Only programs without syntax errors are formatted. As a safeguard, the
//! result is lexed again and must have the same tokens and comments.

use brim_diagnostics::Diagnostic;
use brim_lexer::{Token, TokenKind};

const INDENT: &str = "    ";

/// Formats a Brim source file. Fails with the syntax errors of a program
/// that does not parse.
pub fn format(src: &str) -> Result<String, Vec<Diagnostic>> {
    let (_, diags) = brim_parser::parse(src);
    if brim_diagnostics::has_errors(&diags) {
        return Err(diags);
    }
    let (tokens, _) = brim_lexer::lex(src);
    let lines = split_lines(src, &tokens);
    let out = render(&lines);
    if signature(src) != signature(&out) {
        return Err(vec![
            Diagnostic::error("internal formatter error: formatting would change the program")
                .with_note("this is a bug in the Brim formatter; please report it"),
        ]);
    }
    Ok(out)
}

/// A source line: its tokens and its comment.
struct Line<'a> {
    tokens: Vec<(&'a TokenKind, &'a str)>,
    comment: Option<&'a str>,
}

impl Line<'_> {
    fn is_blank(&self) -> bool {
        self.tokens.is_empty() && self.comment.is_none()
    }
}

/// Splits the token stream into lines. Comments are not tokens: they are
/// found in the text between tokens, where they run to the end of the line.
fn split_lines<'a>(src: &'a str, tokens: &'a [Token]) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut line = Line {
        tokens: Vec::new(),
        comment: None,
    };
    let mut prev_end = 0;
    for token in tokens {
        let gap = &src[prev_end..token.span.start];
        if let Some(at) = gap.find("//") {
            line.comment = Some(gap[at..].trim_end());
        }
        prev_end = token.span.end;
        match token.kind {
            TokenKind::Newline | TokenKind::Eof => {
                lines.push(std::mem::replace(
                    &mut line,
                    Line {
                        tokens: Vec::new(),
                        comment: None,
                    },
                ));
            }
            _ => line
                .tokens
                .push((&token.kind, &src[token.span.start..token.span.end])),
        }
    }
    lines
}

/// An open bracket.
struct Open {
    /// Its contents start on a new line, so they are indented.
    broken: bool,
    /// `{` after `.`, in `import a.{b, c}`: no spaces inside.
    tight: bool,
}

fn render(lines: &[Line<'_>]) -> String {
    let mut out = String::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut pending_blank = false;
    // The last token of the previous line with tokens, if any.
    let mut prev_last: Option<&TokenKind> = None;
    let mut after_open = true;
    for line in lines {
        if line.is_blank() {
            pending_blank = !out.is_empty() && !after_open;
            continue;
        }
        let first = line.tokens.first().map(|t| t.0);
        let closes_block = first.is_some_and(is_close);
        if pending_blank && !closes_block {
            out.push('\n');
        }
        pending_blank = false;

        // Indentation: the broken brackets that are open at the start of
        // the line, without those the line starts by closing.
        let mut depth = stack.iter().filter(|o| o.broken).count();
        let mut open = stack.len();
        for (kind, _) in &line.tokens {
            if !is_close(kind) || open == 0 {
                break;
            }
            open -= 1;
            if stack[open].broken {
                depth -= 1;
            }
        }
        // An expression continues on this line, or the innermost bracket
        // was opened on an earlier line with contents after it.
        if continues(prev_last, first) || open > 0 && !stack[open - 1].broken {
            depth += 1;
        }
        for _ in 0..depth {
            out.push_str(INDENT);
        }

        let mut prev: Option<&TokenKind> = None;
        for (j, (kind, text)) in line.tokens.iter().enumerate() {
            if let Some(p) = prev
                && space_between(p, kind, prev_before(&line.tokens, j), &stack)
            {
                out.push(' ');
            }
            out.push_str(text);
            if is_open(kind) {
                let broken = j + 1 == line.tokens.len();
                let tight = **kind == TokenKind::LBrace && prev == Some(&TokenKind::Dot);
                stack.push(Open { broken, tight });
            } else if is_close(kind) {
                stack.pop();
            }
            prev = Some(kind);
        }
        if let Some(comment) = line.comment {
            if !line.tokens.is_empty() {
                out.push(' ');
            }
            out.push_str(comment);
        }
        out.push('\n');
        if let Some(&(last, _)) = line.tokens.last() {
            prev_last = Some(last);
            after_open = is_open(last);
        } else {
            after_open = false;
        }
    }
    out
}

/// The token before the `j`-th on the line, needed to tell unary from
/// binary `-`.
fn prev_before<'a>(tokens: &[(&'a TokenKind, &str)], j: usize) -> Option<&'a TokenKind> {
    j.checked_sub(2).map(|k| tokens[k].0)
}

fn is_open(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket
    )
}

fn is_close(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket
    )
}

/// Whether a token can end an operand, so that a `-` after it subtracts.
fn ends_value(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Str(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::SelfValue
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
            | TokenKind::Question
    )
}

fn is_binary_operator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::LtEq
            | TokenKind::Gt
            | TokenKind::GtEq
            | TokenKind::AndAnd
            | TokenKind::OrOr
    )
}

/// Whether a line continues the expression of the previous one: the
/// previous line ends with an operator, or this one starts with `.` or a
/// binary operator other than `-`.
fn continues(prev_last: Option<&TokenKind>, first: Option<&TokenKind>) -> bool {
    let after = prev_last.is_some_and(|k| {
        is_binary_operator(k)
            || matches!(
                k,
                TokenKind::Eq
                    | TokenKind::PlusEq
                    | TokenKind::MinusEq
                    | TokenKind::StarEq
                    | TokenKind::SlashEq
                    | TokenKind::PercentEq
                    | TokenKind::Dot
                    | TokenKind::Arrow
                    | TokenKind::FatArrow
            )
    });
    let before = first
        .is_some_and(|k| *k == TokenKind::Dot || (is_binary_operator(k) && *k != TokenKind::Minus));
    after || before
}

/// Whether a space goes between two tokens on a line. `before` is the token
/// before `prev`.
fn space_between(
    prev: &TokenKind,
    next: &TokenKind,
    before: Option<&TokenKind>,
    stack: &[Open],
) -> bool {
    use TokenKind as T;
    let tight = stack.last().is_some_and(|o| o.tight);
    match next {
        T::RParen
        | T::RBracket
        | T::Comma
        | T::Semi
        | T::Question
        | T::Dot
        | T::DotDot
        | T::Colon => return false,
        T::RBrace => return !tight && *prev != T::LBrace,
        _ => {}
    }
    match prev {
        T::LParen | T::LBracket | T::Dot | T::DotDot | T::Bang => return false,
        T::LBrace => return !tight,
        // Unary minus.
        T::Minus if !before.is_some_and(ends_value) => return false,
        _ => {}
    }
    match next {
        // A call, or generic arguments and indexing.
        T::LParen | T::LBracket => !ends_value(prev) || *prev == T::RBrace,
        _ => true,
    }
}

/// What the formatter must not change: the tokens other than newlines, and
/// the comments.
fn signature(src: &str) -> Vec<String> {
    let (tokens, _) = brim_lexer::lex(src);
    let mut out = Vec::new();
    let mut prev_end = 0;
    for token in &tokens {
        let gap = &src[prev_end..token.span.start];
        if let Some(at) = gap.find("//") {
            out.push(gap[at..].trim_end().to_string());
        }
        prev_end = token.span.end;
        if token.kind != TokenKind::Newline {
            out.push(src[token.span.start..token.span.end].to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests;
