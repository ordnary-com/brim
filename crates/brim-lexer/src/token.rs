use std::fmt;

use brim_diagnostics::Span;

/// A lexical token together with its location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Every kind of token Brim recognises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// An identifier such as `main` or `user_name`.
    Ident(String),
    /// A decimal integer literal. Range checking against `i64` happens in
    /// semantic analysis so that `-9223372036854775808` can be written.
    Int(u64),
    /// A string literal, with escape sequences already processed.
    Str(String),

    // Keywords.
    Fn,
    Let,
    Return,
    True,
    False,
    Mut,
    If,
    Else,
    While,
    Break,
    Continue,
    Struct,
    Enum,
    Match,
    For,
    In,
    Impl,
    /// `self`, the receiver of a method.
    SelfValue,
    /// A keyword reserved for a future language feature (`if`, `struct`, ...).
    Reserved(&'static str),

    // Punctuation.
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Arrow,
    Dot,
    /// `..`, in `for i in 0..n`.
    DotDot,
    /// `=>`, between a `match` pattern and its body.
    FatArrow,
    /// `?`, which propagates an error or a missing value.
    Question,

    // Operators.
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AndAnd,
    OrOr,
    Bang,

    /// A character that is not valid Brim. The lexer has already reported
    /// it; the parser skips it without reporting follow-up errors.
    Unknown,
    /// A line break. Newlines terminate statements; see the language spec.
    Newline,
    /// End of input. Always the last token.
    Eof,
}

/// Keywords with a meaning in the current language.
pub const KEYWORDS: &[(&str, TokenKind)] = &[
    ("fn", TokenKind::Fn),
    ("let", TokenKind::Let),
    ("return", TokenKind::Return),
    ("true", TokenKind::True),
    ("false", TokenKind::False),
    ("mut", TokenKind::Mut),
    ("if", TokenKind::If),
    ("else", TokenKind::Else),
    ("while", TokenKind::While),
    ("break", TokenKind::Break),
    ("continue", TokenKind::Continue),
    ("struct", TokenKind::Struct),
    ("enum", TokenKind::Enum),
    ("match", TokenKind::Match),
    ("for", TokenKind::For),
    ("in", TokenKind::In),
    ("impl", TokenKind::Impl),
    ("self", TokenKind::SelfValue),
];

/// Words reserved for planned features. They cannot be used as identifiers,
/// which lets those features be added later without breaking programs.
pub const RESERVED: &[&str] = &[
    "as", "async", "await", "const", "import", "loop", "pub", "trait", "type", "use",
];

impl TokenKind {
    /// A short description used in diagnostics, e.g. "`(`" or "identifier".
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(name) => format!("identifier `{name}`"),
            TokenKind::Int(_) => "integer literal".to_string(),
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::Reserved(word) => format!("reserved keyword `{word}`"),
            TokenKind::Unknown => "invalid character".to_string(),
            TokenKind::Newline => "newline".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{other}`"),
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TokenKind::Ident(name) => name,
            TokenKind::Int(value) => return write!(f, "{value}"),
            TokenKind::Str(value) => return write!(f, "{value:?}"),
            TokenKind::Fn => "fn",
            TokenKind::Let => "let",
            TokenKind::Return => "return",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::Mut => "mut",
            TokenKind::If => "if",
            TokenKind::Else => "else",
            TokenKind::While => "while",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::Struct => "struct",
            TokenKind::Enum => "enum",
            TokenKind::Match => "match",
            TokenKind::For => "for",
            TokenKind::In => "in",
            TokenKind::Impl => "impl",
            TokenKind::SelfValue => "self",
            TokenKind::Reserved(word) => word,
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Colon => ":",
            TokenKind::Semi => ";",
            TokenKind::Arrow => "->",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::FatArrow => "=>",
            TokenKind::Question => "?",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Eq => "=",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::PercentEq => "%=",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::Bang => "!",
            TokenKind::Unknown => "<unknown>",
            TokenKind::Newline => "\\n",
            TokenKind::Eof => "<eof>",
        };
        f.write_str(s)
    }
}
