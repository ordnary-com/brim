//! The Brim lexer: turns UTF-8 source text into [`Token`]s.
//!
//! The lexer never panics on malformed input. Problems are reported as
//! [`Diagnostic`]s and lexing continues, so a single run can report several
//! independent errors. Invalid characters become [`TokenKind::Unknown`]
//! tokens so the parser can avoid reporting them a second time. The token stream always ends with [`TokenKind::Eof`].
//!
//! Newlines are significant in Brim (they terminate statements), so they are
//! emitted as [`TokenKind::Newline`] tokens. Other whitespace and `//` comments
//! are discarded.

mod token;

use brim_diagnostics::{Diagnostic, Span};
pub use token::{KEYWORDS, RESERVED, Token, TokenKind};

/// Lexes `src` into tokens and diagnostics.
pub fn lex(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer {
        src,
        pos: 0,
        tokens: Vec::new(),
        diags: Vec::new(),
    };
    lexer.run();
    (lexer.tokens, lexer.diags)
}

struct Lexer<'a> {
    src: &'a str,
    /// Current byte offset.
    pos: usize,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.src[self.pos..].chars();
        chars.next();
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }

    fn run(&mut self) {
        // A UTF-8 byte order mark at the very start is ignored.
        if self.src.starts_with('\u{feff}') {
            self.pos = '\u{feff}'.len_utf8();
        }
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                '\n' => {
                    self.bump();
                    self.push(TokenKind::Newline, start);
                }
                ' ' | '\t' | '\r' => {
                    self.bump();
                }
                '/' if self.peek_second() == Some('/') => self.line_comment(),
                '"' => self.string(),
                '0'..='9' => self.number(),
                'a'..='z' | 'A'..='Z' | '_' => self.ident_or_keyword(),
                _ => self.punct(c),
            }
        }
        let end = self.src.len();
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::point(end),
        });
    }

    fn line_comment(&mut self) {
        // The newline itself is left for the main loop: it still ends a statement.
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn ident_or_keyword(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_')) {
            self.bump();
        }
        let text = &self.src[start..self.pos];
        let kind = if let Some((_, kw)) = KEYWORDS.iter().find(|(name, _)| *name == text) {
            kw.clone()
        } else if let Some(word) = RESERVED.iter().find(|word| **word == text) {
            TokenKind::Reserved(word)
        } else {
            TokenKind::Ident(text.to_string())
        };
        self.push(kind, start);
    }

    fn number(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some('0'..='9' | '_')) {
            self.bump();
        }
        // A fraction (`1.5`) and an exponent (`2e10`, `1.5e-3`) make a
        // floating-point literal. `0..n` stays a range, and `1.x` a field.
        let rest = &self.src[self.pos..];
        let fraction = rest.starts_with('.') && rest[1..].starts_with(|c: char| c.is_ascii_digit());
        if fraction {
            self.bump();
            while matches!(self.peek(), Some('0'..='9' | '_')) {
                self.bump();
            }
        }
        let rest = &self.src[self.pos..];
        let after_e = rest.get(1..).unwrap_or("");
        let after_e = after_e
            .strip_prefix('+')
            .or_else(|| after_e.strip_prefix('-'))
            .unwrap_or(after_e);
        let exponent =
            rest.starts_with(['e', 'E']) && after_e.starts_with(|c: char| c.is_ascii_digit());
        if exponent {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while matches!(self.peek(), Some('0'..='9' | '_')) {
                self.bump();
            }
        }
        if fraction || exponent {
            let text: String = self.src[start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let value = text.parse::<f64>().unwrap_or(f64::INFINITY);
            if value.is_infinite() {
                self.diags.push(
                    Diagnostic::error("floating-point literal is too large")
                        .with_code("E0007")
                        .with_primary(Span::new(start, self.pos), "")
                        .with_note(format!("the largest `f64` is about {:e}", f64::MAX)),
                );
            }
            self.push(TokenKind::Float(value.to_bits()), start);
            return;
        }
        // Letters directly after a number (`10px`, `0x1F`) are not supported.
        if matches!(self.peek(), Some('a'..='z' | 'A'..='Z')) {
            let suffix_start = self.pos;
            while matches!(self.peek(), Some('a'..='z' | 'A'..='Z' | '0'..='9' | '_')) {
                self.bump();
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "invalid suffix `{}` on integer literal",
                    &self.src[suffix_start..self.pos]
                ))
                .with_code("E0006")
                .with_primary(Span::new(start, self.pos), "invalid integer literal")
                .with_note("only decimal integer literals are supported"),
            );
            self.push(TokenKind::Int(0), start);
            return;
        }
        let text = &self.src[start..self.pos];
        let digits: String = text.chars().filter(|c| *c != '_').collect();
        if digits.len() > 1 && digits.starts_with('0') {
            let trimmed = digits.trim_start_matches('0');
            let suggestion = if trimmed.is_empty() { "0" } else { trimmed };
            self.diags.push(
                Diagnostic::error("integer literals cannot have leading zeros")
                    .with_code("E0005")
                    .with_primary(Span::new(start, self.pos), "")
                    .with_help(format!("write `{suggestion}` instead")),
            );
        }
        let value = match digits.parse::<u64>() {
            Ok(value) => value,
            Err(_) => {
                self.diags.push(
                    Diagnostic::error("integer literal is too large")
                        .with_code("E0004")
                        .with_primary(Span::new(start, self.pos), "")
                        .with_note(format!(
                            "the largest integer type is `i64` (maximum {})",
                            i64::MAX
                        )),
                );
                0
            }
        };
        self.push(TokenKind::Int(value), start);
    }

    fn string(&mut self) {
        let start = self.pos;
        self.bump(); // opening quote
        let mut value = String::new();
        loop {
            let Some(c) = self.peek() else {
                self.unterminated_string(start);
                break;
            };
            match c {
                '"' => {
                    self.bump();
                    break;
                }
                '\n' => {
                    self.unterminated_string(start);
                    break;
                }
                '\\' => {
                    if let Some(decoded) = self.escape() {
                        value.push(decoded);
                    }
                }
                _ => {
                    self.bump();
                    value.push(c);
                }
            }
        }
        self.push(TokenKind::Str(value), start);
    }

    fn unterminated_string(&mut self, start: usize) {
        self.diags.push(
            Diagnostic::error("unterminated string literal")
                .with_code("E0002")
                .with_primary(Span::new(start, start + 1), "string starts here")
                .with_note("string literals must end with `\"` on the same line")
                .with_help("use `\\n` to put a line break inside a string"),
        );
    }

    /// Lexes an escape sequence starting at a backslash. Returns `None` after
    /// reporting an invalid escape.
    fn escape(&mut self) -> Option<char> {
        let start = self.pos;
        self.bump(); // backslash
        let c = match self.peek() {
            None | Some('\n') => {
                self.invalid_escape(start, "incomplete escape sequence");
                return None;
            }
            Some(c) => c,
        };
        self.bump();
        match c {
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            '0' => Some('\0'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            'u' => self.unicode_escape(start),
            _ => {
                self.invalid_escape(start, &format!("unknown escape sequence `\\{c}`"));
                None
            }
        }
    }

    /// Lexes the `{XXXX}` part of a `\u{XXXX}` escape.
    fn unicode_escape(&mut self, start: usize) -> Option<char> {
        if !self.eat('{') {
            self.invalid_escape(start, "expected `{` after `\\u`");
            return None;
        }
        let digits_start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
            self.bump();
        }
        let digits = &self.src[digits_start..self.pos];
        if !self.eat('}') {
            self.invalid_escape(start, "unicode escape must end with `}`");
            return None;
        }
        let value = if digits.is_empty() || digits.len() > 6 {
            None
        } else {
            u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32)
        };
        if value.is_none() {
            self.invalid_escape(start, "invalid unicode escape");
        }
        value
    }

    fn invalid_escape(&mut self, start: usize, message: &str) {
        self.diags.push(
            Diagnostic::error(message.to_string())
                .with_code("E0003")
                .with_primary(Span::new(start, self.pos), "")
                .with_note(
                    "supported escapes: \\n \\r \\t \\0 \\\\ \\\" and \\u{...} (1-6 hex digits)",
                ),
        );
    }

    fn punct(&mut self, c: char) {
        let start = self.pos;
        self.bump();
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semi,
            '.' if self.eat('.') => TokenKind::DotDot,
            '.' => TokenKind::Dot,
            '?' => TokenKind::Question,
            '+' if self.eat('=') => TokenKind::PlusEq,
            '+' => TokenKind::Plus,
            '*' if self.eat('=') => TokenKind::StarEq,
            '*' => TokenKind::Star,
            '/' if self.eat('=') => TokenKind::SlashEq,
            '/' => TokenKind::Slash,
            '%' if self.eat('=') => TokenKind::PercentEq,
            '%' => TokenKind::Percent,
            '-' if self.eat('>') => TokenKind::Arrow,
            '-' if self.eat('=') => TokenKind::MinusEq,
            '-' => TokenKind::Minus,
            '=' if self.eat('=') => TokenKind::EqEq,
            '=' if self.eat('>') => TokenKind::FatArrow,
            '=' => TokenKind::Eq,
            '!' if self.eat('=') => TokenKind::BangEq,
            '!' => TokenKind::Bang,
            '<' if self.eat('=') => TokenKind::LtEq,
            '<' => TokenKind::Lt,
            '>' if self.eat('=') => TokenKind::GtEq,
            '>' => TokenKind::Gt,
            '&' if self.eat('&') => TokenKind::AndAnd,
            '|' if self.eat('|') => TokenKind::OrOr,
            '|' => TokenKind::Pipe,
            _ => {
                self.unexpected_char(c, start);
                TokenKind::Unknown
            }
        };
        self.push(kind, start);
    }

    fn unexpected_char(&mut self, c: char, start: usize) {
        let mut diag = Diagnostic::error(format!("unexpected character `{}`", c.escape_debug()))
            .with_code("E0001")
            .with_primary(Span::new(start, self.pos), "");
        diag = match c {
            '&' => diag.with_help("logical AND is written `&&`"),
            '\'' => diag.with_help("string literals use double quotes: \"...\""),
            '#' => diag.with_help("comments start with `//`"),
            c if c.is_alphabetic() => {
                diag.with_note("identifiers may only contain ASCII letters, digits and `_`")
            }
            _ => diag,
        };
        self.diags.push(diag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brim_diagnostics::SourceFile;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (tokens, diags) = lex(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        tokens.into_iter().map(|t| t.kind).collect()
    }

    fn errors(src: &str) -> Vec<(&'static str, String)> {
        let (_, diags) = lex(src);
        diags
            .into_iter()
            .map(|d| (d.code.unwrap().0, d.message))
            .collect()
    }

    fn ident(s: &str) -> TokenKind {
        TokenKind::Ident(s.to_string())
    }

    #[test]
    fn empty_input_is_just_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn lexes_function_header() {
        use TokenKind::*;
        assert_eq!(
            kinds("fn add(a: i64, b: i64) -> i64 {"),
            vec![
                Fn,
                ident("add"),
                LParen,
                ident("a"),
                Colon,
                ident("i64"),
                Comma,
                ident("b"),
                Colon,
                ident("i64"),
                RParen,
                Arrow,
                ident("i64"),
                LBrace,
                Eof
            ]
        );
    }

    #[test]
    fn lexes_all_operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("+ - * / % = == != < <= > >= && || | ! ; += -= *= /= %="),
            vec![
                Plus, Minus, Star, Slash, Percent, Eq, EqEq, BangEq, Lt, LtEq, Gt, GtEq, AndAnd,
                OrOr, Pipe, Bang, Semi, PlusEq, MinusEq, StarEq, SlashEq, PercentEq, Eof
            ]
        );
    }

    #[test]
    fn keywords_and_reserved_words() {
        use TokenKind::*;
        assert_eq!(
            kinds("let mut return true false if else while break continue struct letter"),
            vec![
                Let,
                Mut,
                Return,
                True,
                False,
                If,
                Else,
                While,
                Break,
                Continue,
                Struct,
                ident("letter"),
                Eof
            ]
        );
    }

    #[test]
    fn integers_with_separators() {
        assert_eq!(
            kinds("0 42 1_000_000 18446744073709551615"),
            vec![
                TokenKind::Int(0),
                TokenKind::Int(42),
                TokenKind::Int(1_000_000),
                TokenKind::Int(u64::MAX),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn integer_errors() {
        assert_eq!(
            errors("18446744073709551616"),
            vec![("E0004", "integer literal is too large".to_string())]
        );
        assert_eq!(
            errors("007"),
            vec![(
                "E0005",
                "integer literals cannot have leading zeros".to_string()
            )]
        );
        assert_eq!(
            errors("0x1F"),
            vec![(
                "E0006",
                "invalid suffix `x1F` on integer literal".to_string()
            )]
        );
        assert_eq!(
            errors("1e999"),
            vec![("E0007", "floating-point literal is too large".to_string())]
        );
        use TokenKind::{Dot, DotDot, Eof, Float, Int};
        assert_eq!(
            kinds("1.5 2e3 1.5E-2 0..3 x.0"),
            vec![
                Float(1.5f64.to_bits()),
                Float(2000f64.to_bits()),
                Float(0.015f64.to_bits()),
                Int(0),
                DotDot,
                Int(3),
                ident("x"),
                Dot,
                Int(0),
                Eof
            ]
        );
    }

    #[test]
    fn field_access_and_structs() {
        assert_eq!(
            kinds("struct p.x"),
            vec![
                TokenKind::Struct,
                TokenKind::Ident("p".into()),
                TokenKind::Dot,
                TokenKind::Ident("x".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(
            kinds(r#""hi" "a\nb\t\"q\"\\" "\u{1F600}" "héllo""#),
            vec![
                TokenKind::Str("hi".into()),
                TokenKind::Str("a\nb\t\"q\"\\".into()),
                TokenKind::Str("😀".into()),
                TokenKind::Str("héllo".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn comments_are_skipped_but_newlines_kept() {
        use TokenKind::*;
        assert_eq!(
            kinds("let // a comment with \"quotes\"\n// whole line\nx"),
            vec![Let, Newline, Newline, ident("x"), Eof]
        );
    }

    #[test]
    fn unterminated_string_does_not_panic() {
        let (tokens, diags) = lex("print(\"hello\n)");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.unwrap().0, "E0002");
        assert_eq!(diags[0].span(), Some(Span::new(6, 7)));
        // Lexing continues on the next line.
        assert!(tokens.iter().any(|t| t.kind == TokenKind::RParen));
        assert_eq!(errors("\"abc").len(), 1);
    }

    #[test]
    fn invalid_escapes() {
        assert_eq!(
            errors(r#""\q""#),
            vec![("E0003", "unknown escape sequence `\\q`".to_string())]
        );
        assert_eq!(errors(r#""\u{110000}""#)[0].0, "E0003");
        assert_eq!(errors(r#""\u{}""#)[0].0, "E0003");
        assert_eq!(errors(r#""\u12""#)[0].0, "E0003");
        assert_eq!(errors("\"\\").len(), 2); // incomplete escape + unterminated
    }

    #[test]
    fn invalid_characters_are_reported_and_skipped() {
        let (tokens, diags) = lex("a @ b # é &");
        let codes: Vec<_> = diags.iter().map(|d| d.code.unwrap().0).collect();
        assert_eq!(codes, vec!["E0001"; 4]);
        assert_eq!(diags[0].message, "unexpected character `@`");
        assert_eq!(diags[3].help, vec!["logical AND is written `&&`"]);
        let kinds: Vec<_> = tokens.into_iter().map(|t| t.kind).collect();
        use TokenKind::Unknown;
        assert_eq!(
            kinds,
            vec![
                ident("a"),
                Unknown,
                ident("b"),
                Unknown,
                Unknown,
                Unknown,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn spans_map_to_lines_and_columns() {
        let text = "fn main() {\n    let s: string = \"é\"\n    s\n}";
        let file = SourceFile::new("t.brim", text);
        let (tokens, _) = lex(text);
        let s_use = tokens
            .iter()
            .filter(|t| t.kind == ident("s"))
            .nth(1)
            .unwrap();
        assert_eq!(file.slice(s_use.span), "s");
        let loc = file.location(s_use.span.start);
        assert_eq!((loc.line, loc.column), (3, 5));

        let string = tokens
            .iter()
            .find(|t| matches!(t.kind, TokenKind::Str(_)))
            .unwrap();
        assert_eq!(file.slice(string.span), "\"é\"");
        assert_eq!(string.span.len(), 4); // bytes, not characters
    }

    #[test]
    fn byte_order_mark_and_crlf() {
        use TokenKind::*;
        assert_eq!(
            kinds("\u{feff}let\r\nx"),
            vec![Let, Newline, ident("x"), Eof]
        );
    }

    #[test]
    fn eof_span_is_at_end() {
        let (tokens, _) = lex("ab ");
        assert_eq!(tokens.last().unwrap().span, Span::point(3));
    }
}
