//! Recursive-descent parser for Brim.
//!
//! Statements and items are parsed by straightforward recursive descent;
//! expressions use precedence climbing (see [`brim_ast::BinaryOp::precedence`]).
//!
//! # Newlines
//!
//! Newlines terminate statements. They are ignored:
//! - inside parentheses (argument lists, parameter lists, grouping), and
//! - directly after a binary operator, `=`, `->` or `,`,
//!
//! so long expressions can be continued by ending a line with an operator.
//!
//! # Errors
//!
//! The parser never panics on malformed input. It records a [`Diagnostic`],
//! then skips to the next statement (or the next `fn` at the top level) and
//! continues, so several independent syntax errors can be reported at once.
//! Nesting of blocks and expressions is limited to [`MAX_NESTING`] levels so
//! that adversarial input cannot overflow the stack in this or later compiler
//! phases.

use brim_ast::{
    BinaryOp, Block, ConstDecl, EnumDecl, Expr, ExprKind, FStringPart, FieldDecl, FieldInit,
    FnDecl, ForIter, Ident, ImplDecl, ImportDecl, MatchArm, Param, Pattern, PatternKind, PlaceElem,
    Program, Receiver, Stmt, StmtKind, StructDecl, TraitDecl, TraitMethod, TypeExpr, UnaryOp,
    VariantDecl,
};
use brim_diagnostics::{Diagnostic, Span};
use brim_lexer::{Token, TokenKind, lex};

/// Maximum nesting depth of blocks and expressions (including long operator
/// chains and `else if` chains).
pub const MAX_NESTING: usize = 128;

/// Lexes and parses `src`. Lexer and parser diagnostics are returned together,
/// in source order.
pub fn parse(src: &str) -> (Program, Vec<Diagnostic>) {
    let (tokens, mut diags) = lex(src);
    let (program, parse_diags) = parse_tokens(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
    (program, diags)
}

/// Lexes and parses `src` as a file whose spans start at `base` in a
/// [`SourceMap`](brim_diagnostics::SourceMap).
pub fn parse_at(src: &str, base: usize) -> (Program, Vec<Diagnostic>) {
    let (mut tokens, lex_diags) = lex(src);
    for token in &mut tokens {
        token.span = Span::new(token.span.start + base, token.span.end + base);
    }
    let mut diags: Vec<Diagnostic> = lex_diags.into_iter().map(|d| d.shift(base)).collect();
    let (program, parse_diags) = parse_tokens(tokens);
    diags.extend(parse_diags);
    diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
    (program, diags)
}

/// Parses an already-lexed token stream. The stream must end with
/// [`TokenKind::Eof`], as produced by [`brim_lexer::lex`].
pub fn parse_tokens(mut tokens: Vec<Token>) -> (Program, Vec<Diagnostic>) {
    if tokens.last().is_none_or(|t| t.kind != TokenKind::Eof) {
        let end = tokens.last().map_or(0, |t| t.span.end);
        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::point(end),
        });
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        paren_depth: 0,
        nesting: 0,
        no_struct_literal: false,
        diags: Vec::new(),
    };
    let program = parser.program();
    (program, parser.diags)
}

/// `a` or `a.b.c` as one dotted name, if `expr` is only names and fields.
fn dotted_name(expr: &Expr) -> Option<Ident> {
    match &expr.kind {
        ExprKind::Name(name) => Some(name.clone()),
        ExprKind::Field { base, field } => {
            let base = dotted_name(base)?;
            Some(Ident {
                name: format!("{}.{}", base.name, field.name),
                span: base.span.to(field.span),
            })
        }
        _ => None,
    }
}

/// Marker for "an error was already reported". Callers recover and continue.
#[derive(Debug)]
struct Reported;

/// A function's signature, before its body.
struct FnHeader {
    fn_token: Token,
    name: Ident,
    type_params: Vec<Ident>,
    bounds: Vec<Vec<Ident>>,
    receiver: Option<Receiver>,
    params: Vec<Param>,
    ret: Option<TypeExpr>,
    /// Where the signature ends.
    end: Span,
}

type PResult<T> = Result<T, Reported>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// While positive, newline tokens are skipped.
    paren_depth: usize,
    /// Current expression nesting depth.
    nesting: usize,
    /// Set while parsing an `if` or `while` condition, where `Name {` starts
    /// the body rather than a struct literal. Cleared inside parentheses.
    no_struct_literal: bool,
    diags: Vec<Diagnostic>,
}

impl Parser {
    // ---- token access -------------------------------------------------

    fn current_index(&self) -> usize {
        let mut i = self.pos;
        if self.paren_depth > 0 {
            // Eof is never a newline, so this stays in bounds.
            while self.tokens[i].kind == TokenKind::Newline {
                i += 1;
            }
        }
        i
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current_index()]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn bump(&mut self) -> Token {
        let i = self.current_index();
        let token = self.tokens[i].clone();
        if token.kind != TokenKind::Eof {
            self.pos = i + 1;
        }
        token
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<Token> {
        self.at(kind).then(|| self.bump())
    }

    fn skip_newlines(&mut self) {
        while self.tokens[self.pos].kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> PResult<Token> {
        if let Some(token) = self.eat(kind) {
            return Ok(token);
        }
        let expected = kind.describe();
        Err(self.error_unexpected(&expected))
    }

    fn expect_ident(&mut self, what: &str) -> PResult<Ident> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(Ident {
                    name,
                    span: token.span,
                })
            }
            TokenKind::Reserved(word) => Err(self.reserved_keyword(word, token.span)),
            _ => Err(self.error_unexpected(what)),
        }
    }

    /// Runs `f` with newlines ignored, restoring the previous mode afterwards.
    fn in_parens<T>(&mut self, f: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.paren_depth += 1;
        let no_struct = std::mem::replace(&mut self.no_struct_literal, false);
        let result = f(self);
        self.no_struct_literal = no_struct;
        self.paren_depth -= 1;
        result
    }

    // ---- diagnostics --------------------------------------------------

    fn report(&mut self, diag: Diagnostic) -> Reported {
        self.diags.push(diag);
        Reported
    }

    /// Reports "expected X, found Y" at the current token.
    fn error_unexpected(&mut self, expected: &str) -> Reported {
        let found = self.peek().clone();
        if found.kind == TokenKind::Unknown {
            // Already reported by the lexer.
            return Reported;
        }
        let mut diag = Diagnostic::error(format!(
            "expected {expected}, found {}",
            found.kind.describe()
        ))
        .with_code("E0100")
        .with_primary(found.span, format!("expected {expected}"));
        if let TokenKind::Reserved(word) = found.kind {
            diag = diag.with_note(format!(
                "`{word}` is a reserved keyword and cannot be used as a name"
            ));
        }
        self.report(diag)
    }

    // Error constructors are kept out of line so the recursive expression
    // parsing functions keep small stack frames.

    #[cold]
    #[inline(never)]
    fn expected_expression(&mut self, token: &Token) -> Reported {
        if token.kind == TokenKind::Unknown {
            return Reported;
        }
        let mut diag = Diagnostic::error(format!(
            "expected an expression, found {}",
            token.kind.describe()
        ))
        .with_code("E0100")
        .with_primary(token.span, "expected an expression");
        if Self::binary_op(&token.kind).is_some() {
            diag = diag.with_note(
                "to continue an expression on the next line, end the previous line with the operator",
            );
        }
        self.report(diag)
    }

    #[cold]
    #[inline(never)]
    fn chained_comparison(&mut self, span: Span, previous: Span) -> Reported {
        let diag = Diagnostic::error("comparison operators cannot be chained")
            .with_code("E0106")
            .with_primary(span, "")
            .with_secondary(previous, "previous comparison")
            .with_help("combine the comparisons with `&&`, e.g. `a < b && b < c`");
        self.report(diag)
    }

    #[cold]
    #[inline(never)]
    fn reserved_keyword(&mut self, word: &str, span: Span) -> Reported {
        let diag = Diagnostic::error(format!("`{word}` is not supported yet"))
            .with_code("E0101")
            .with_primary(span, "reserved keyword")
            .with_note(format!(
                "`{word}` is reserved for a future version of Brim and cannot be used as a name"
            ));
        self.report(diag)
    }

    // ---- recovery -----------------------------------------------------

    /// Skips to the start of the next top-level item.
    fn recover_item(&mut self) {
        self.paren_depth = 0;
        self.no_struct_literal = false;
        while !matches!(
            self.peek_kind(),
            TokenKind::Fn
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::Impl
                | TokenKind::Trait
                | TokenKind::Const
                | TokenKind::Pub
                | TokenKind::Import
                | TokenKind::Eof
        ) {
            self.bump();
        }
    }

    /// Skips the rest of the current statement. Stops after a newline or `;`,
    /// or before a `}` closing the enclosing block or a `fn`.
    fn recover_stmt(&mut self) {
        self.paren_depth = 0;
        self.no_struct_literal = false;
        let mut braces = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::Eof | TokenKind::Fn => return,
                TokenKind::Newline | TokenKind::Semi if braces == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::LBrace => braces += 1,
                TokenKind::RBrace if braces == 0 => return,
                TokenKind::RBrace => braces -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    // ---- items --------------------------------------------------------

    fn program(&mut self) -> Program {
        let mut functions = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut impls = Vec::new();
        let mut traits = Vec::new();
        let mut consts = Vec::new();
        let mut imports = Vec::new();
        loop {
            while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Semi) {
                self.bump();
            }
            let token = self.peek().clone();
            match &token.kind {
                TokenKind::Eof => break,
                TokenKind::Import => match self.import_decl() {
                    Ok(i) => imports.push(i),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Pub => {
                    self.bump();
                    match self.peek_kind() {
                        TokenKind::Fn => match self.function() {
                            Ok(mut f) => {
                                f.public = true;
                                functions.push(f);
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        TokenKind::Struct => match self.struct_decl() {
                            Ok(mut s) => {
                                s.public = true;
                                structs.push(s);
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        TokenKind::Enum => match self.enum_decl() {
                            Ok(mut e) => {
                                e.public = true;
                                enums.push(e);
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        TokenKind::Trait => match self.trait_decl() {
                            Ok(mut t) => {
                                t.public = true;
                                traits.push(t);
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        TokenKind::Const => match self.const_decl() {
                            Ok(mut c) => {
                                c.public = true;
                                consts.push(c);
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        // `pub import a.{X}` re-exports `X`.
                        TokenKind::Import => match self.import_decl() {
                            Ok(mut i) if i.items.is_some() => {
                                i.public = true;
                                imports.push(i);
                            }
                            Ok(i) => {
                                self.report(
                                    Diagnostic::error("`pub import` re-exports items, not modules")
                                        .with_code("E0100")
                                        .with_primary(i.span, "")
                                        .with_help("name the items: `pub import module.{Item}`"),
                                );
                            }
                            Err(Reported) => self.recover_item(),
                        },
                        _ => {
                            self.error_unexpected(
                                "`fn`, `struct`, `enum`, `trait`, `const` or `import` after `pub`",
                            );
                            self.recover_item();
                        }
                    }
                }
                TokenKind::Fn => match self.function() {
                    Ok(f) => functions.push(f),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Struct => match self.struct_decl() {
                    Ok(s) => structs.push(s),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Enum => match self.enum_decl() {
                    Ok(e) => enums.push(e),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Trait => match self.trait_decl() {
                    Ok(t) => traits.push(t),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Const => match self.const_decl() {
                    Ok(c) => consts.push(c),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Impl => match self.impl_decl() {
                    Ok(i) => impls.push(i),
                    Err(Reported) => self.recover_item(),
                },
                TokenKind::Let => {
                    self.report(
                        Diagnostic::error("variables cannot be declared outside a function")
                            .with_code("E0103")
                            .with_primary(token.span, "")
                            .with_note("global variables and constants are not supported yet"),
                    );
                    self.bump();
                    self.recover_item();
                }
                TokenKind::Reserved(word) => {
                    self.reserved_keyword(word, token.span);
                    self.bump();
                    self.recover_item();
                }
                TokenKind::Unknown => {
                    self.bump();
                    self.recover_item();
                }
                _ => {
                    let diag = Diagnostic::error(format!(
                        "expected `fn`, `struct`, `enum`, `trait`, `const` or `impl`, found {}",
                        token.kind.describe()
                    ))
                    .with_code("E0103")
                    .with_primary(token.span, "expected a declaration")
                    .with_note(
                        "only functions, structs, enums, traits, constants and `impl` blocks can be declared at the top level",
                    );
                    self.report(diag);
                    self.bump();
                    self.recover_item();
                }
            }
        }
        Program {
            imports,
            structs,
            enums,
            impls,
            traits,
            consts,
            functions,
        }
    }

    /// `import a.b`, `import a.b as c` or `import a.b.{X, y}`.
    fn import_decl(&mut self) -> PResult<ImportDecl> {
        let import_token = self.bump();
        let mut path = vec![self.expect_ident("a module name")?];
        let mut items = None;
        let mut end = path[0].span;
        while self.eat(&TokenKind::Dot).is_some() {
            if self.at(&TokenKind::LBrace) {
                self.bump();
                let (names, close) = self.in_parens(|p| {
                    let mut names = Vec::new();
                    while !p.at(&TokenKind::RBrace) {
                        names.push(p.expect_ident("a name to import")?);
                        if p.eat(&TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                    if !p.at(&TokenKind::RBrace) {
                        return Err(p.error_unexpected("`,` or `}`"));
                    }
                    Ok((names, p.bump()))
                })?;
                items = Some(names);
                end = close.span;
                break;
            }
            let part = self.expect_ident("a module name or `{` after `.`")?;
            end = part.span;
            path.push(part);
        }
        let alias = if items.is_none() && self.eat(&TokenKind::As).is_some() {
            let alias = self.expect_ident("a name after `as`")?;
            end = alias.span;
            Some(alias)
        } else {
            None
        };
        Ok(ImportDecl {
            path,
            alias,
            items,
            public: false,
            span: import_token.span.to(end),
        })
    }

    fn impl_decl(&mut self) -> PResult<ImplDecl> {
        let impl_token = self.expect(&TokenKind::Impl)?;
        let first = self.dotted_ident("a struct or enum name")?;
        // `impl Trait for Type`
        let (trait_name, type_name) = if self.eat(&TokenKind::For).is_some() {
            (Some(first), self.expect_ident("a struct or enum name")?)
        } else {
            (None, first)
        };
        let type_params = self.type_params()?;
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the methods"));
        }
        self.bump();
        let mut methods = Vec::new();
        let close = loop {
            while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Semi) {
                self.bump();
            }
            let public = self.eat(&TokenKind::Pub).is_some();
            match self.peek_kind() {
                TokenKind::RBrace if !public => break self.bump(),
                TokenKind::Fn => match self.function() {
                    Ok(mut f) => {
                        f.public = public;
                        methods.push(f);
                    }
                    Err(Reported) => {
                        // Skip to the next method or the end of the block.
                        let mut depth = 0usize;
                        loop {
                            match self.peek_kind() {
                                TokenKind::Eof => break,
                                TokenKind::Fn if depth == 0 => break,
                                TokenKind::LBrace => depth += 1,
                                TokenKind::RBrace if depth == 0 => break,
                                TokenKind::RBrace => depth -= 1,
                                _ => {}
                            }
                            self.bump();
                        }
                    }
                },
                TokenKind::Eof => {
                    return Err(self.error_unexpected("`fn` or `}` in the `impl` block"));
                }
                _ => return Err(self.error_unexpected("`fn` or `}` in the `impl` block")),
            }
        };
        Ok(ImplDecl {
            span: impl_token.span.to(close.span),
            trait_name,
            type_name,
            type_params,
            methods,
        })
    }

    fn enum_decl(&mut self) -> PResult<EnumDecl> {
        let enum_token = self.expect(&TokenKind::Enum)?;
        let name = self.expect_ident("an enum name")?;
        let type_params = self.type_params()?;
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the enum's variants"));
        }
        self.bump();
        let mut variants = Vec::new();
        let close = loop {
            while matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma
            ) {
                self.bump();
            }
            if let Some(close) = self.eat(&TokenKind::RBrace) {
                break close;
            }
            let variant = self.expect_ident("a variant name or `}`")?;
            let mut span = variant.span;
            let mut fields = Vec::new();
            if self.at(&TokenKind::LParen) {
                self.bump();
                let close = self.in_parens(|p| {
                    while !p.at(&TokenKind::RParen) {
                        fields.push(p.type_expr()?);
                        if p.eat(&TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                    if !p.at(&TokenKind::RParen) {
                        return Err(p.error_unexpected("`,` or `)`"));
                    }
                    Ok(p.bump())
                })?;
                span = span.to(close.span);
            }
            variants.push(VariantDecl {
                name: variant,
                fields,
                span,
            });
            if !matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma | TokenKind::RBrace
            ) {
                return Err(self.error_unexpected("a newline, `,` or `}` after the variant"));
            }
        };
        Ok(EnumDecl {
            public: false,
            span: enum_token.span.to(close.span),
            name,
            type_params,
            variants,
        })
    }

    fn struct_decl(&mut self) -> PResult<StructDecl> {
        let struct_token = self.expect(&TokenKind::Struct)?;
        let name = self.expect_ident("a struct name")?;
        let type_params = self.type_params()?;
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the struct's fields"));
        }
        self.bump();
        let mut fields = Vec::new();
        let close = loop {
            while matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma
            ) {
                self.bump();
            }
            if let Some(close) = self.eat(&TokenKind::RBrace) {
                break close;
            }
            let public = self.eat(&TokenKind::Pub).is_some();
            let field_name = self.expect_ident("a field name or `}`")?;
            if !self.at(&TokenKind::Colon) {
                let diag = Diagnostic::error(format!("field `{}` needs a type", field_name.name))
                    .with_code("E0102")
                    .with_primary(field_name.span, "missing type annotation")
                    .with_help(format!(
                        "write `{}: i64` (or another type)",
                        field_name.name
                    ));
                return Err(self.report(diag));
            }
            self.bump();
            let ty = self.type_expr()?;
            fields.push(FieldDecl {
                span: field_name.span.to(ty.span()),
                public,
                name: field_name,
                ty,
            });
            if !matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma | TokenKind::RBrace
            ) {
                return Err(self.error_unexpected("a newline, `,` or `}` after the field"));
            }
        };
        Ok(StructDecl {
            public: false,
            span: struct_token.span.to(close.span),
            name,
            type_params,
            fields,
        })
    }

    /// `fn name[T: Bound](self, params) -> ret`, up to the body.
    fn fn_header(&mut self) -> PResult<FnHeader> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident("a function name")?;
        let (type_params, bounds) = self.type_params_with_bounds()?;
        self.expect(&TokenKind::LParen)?;
        let mut receiver = None;
        let params = self.in_parens(|p| {
            let mut params = Vec::new();
            // `self` or `mut self` first makes this a method.
            let mutable = p.at(&TokenKind::Mut);
            let receiver_start = p.peek().span;
            if mutable {
                p.bump();
            }
            if p.at(&TokenKind::SelfValue) {
                let token = p.bump();
                receiver = Some(Receiver {
                    mutable,
                    span: receiver_start.to(token.span),
                });
                if p.eat(&TokenKind::Comma).is_none() && !p.at(&TokenKind::RParen) {
                    return Err(p.error_unexpected("`,` or `)`"));
                }
            } else if mutable {
                return Err(p.error_unexpected("`self` after `mut`"));
            }
            while !p.at(&TokenKind::RParen) {
                params.push(p.param()?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RParen) {
                return Err(p.error_unexpected("`,` or `)`"));
            }
            let close = p.bump();
            Ok((params, close))
        })?;
        let (params, close) = params;
        let mut end = close.span;
        let ret = if self.eat(&TokenKind::Arrow).is_some() {
            self.skip_newlines();
            let ret = self.type_expr()?;
            end = ret.span();
            Some(ret)
        } else {
            None
        };
        Ok(FnHeader {
            fn_token,
            name,
            type_params,
            bounds,
            receiver,
            params,
            ret,
            end,
        })
    }

    fn function(&mut self) -> PResult<FnDecl> {
        let FnHeader {
            fn_token,
            name,
            type_params,
            bounds,
            receiver,
            params,
            ret,
            ..
        } = self.fn_header()?;
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the function body"));
        }
        let body = self.block()?;
        Ok(FnDecl {
            public: false,
            span: fn_token.span.to(body.span),
            name,
            type_params,
            bounds,
            receiver,
            params,
            ret,
            body,
        })
    }

    fn param(&mut self) -> PResult<Param> {
        let name = self.expect_ident("a parameter name")?;
        if !self.at(&TokenKind::Colon) {
            let diag = Diagnostic::error(format!("parameter `{}` needs a type", name.name))
                .with_code("E0102")
                .with_primary(name.span, "missing type annotation")
                .with_help(format!("write `{}: i64` (or another type)", name.name));
            return Err(self.report(diag));
        }
        self.bump();
        let ty = self.type_expr()?;
        Ok(Param {
            span: name.span.to(ty.span()),
            name,
            ty,
        })
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        if self.at(&TokenKind::Fn) {
            return self.fn_type();
        }
        if self.at(&TokenKind::LParen) {
            return self.tuple_type();
        }
        let mut name = self.expect_ident("a type")?;
        // `module.Type`
        while self.eat(&TokenKind::Dot).is_some() {
            let part = self.expect_ident("a type name after `.`")?;
            name = Ident {
                name: format!("{}.{}", name.name, part.name),
                span: name.span.to(part.span),
            };
        }
        let mut span = name.span;
        let mut args = Vec::new();
        if self.at(&TokenKind::LBracket) {
            self.bump();
            self.enter(0, span)?;
            self.nesting += 1;
            let close = self.in_parens(|p| {
                while !p.at(&TokenKind::RBracket) {
                    args.push(p.type_expr()?);
                    if p.eat(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
                if !p.at(&TokenKind::RBracket) {
                    return Err(p.error_unexpected("`,` or `]`"));
                }
                Ok(p.bump())
            });
            self.nesting -= 1;
            span = span.to(close?.span);
        }
        Ok(TypeExpr {
            name,
            args,
            ret: None,
            span,
        })
    }

    /// `(A, B, ...)`: a tuple type, named `()` in the syntax tree.
    fn tuple_type(&mut self) -> PResult<TypeExpr> {
        let open = self.expect(&TokenKind::LParen)?;
        self.enter(0, open.span)?;
        self.nesting += 1;
        let result = self.in_parens(|p| {
            let mut args = Vec::new();
            while !p.at(&TokenKind::RParen) {
                args.push(p.type_expr()?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RParen) {
                return Err(p.error_unexpected("`,` or `)`"));
            }
            Ok((args, p.bump()))
        });
        self.nesting -= 1;
        let (args, close) = result?;
        let span = open.span.to(close.span);
        if args.len() < 2 {
            let diag = Diagnostic::error("a tuple type has two or more types")
                .with_code("E0100")
                .with_primary(span, "")
                .with_help("write `(A, B)`, or just the type for one value");
            return Err(self.report(diag));
        }
        Ok(TypeExpr {
            name: Ident {
                name: "()".into(),
                span: open.span,
            },
            args,
            ret: None,
            span,
        })
    }

    /// `fn(A, B) -> R`, or `fn(A)` for a function that returns nothing.
    fn fn_type(&mut self) -> PResult<TypeExpr> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        self.expect(&TokenKind::LParen)?;
        self.enter(0, fn_token.span)?;
        self.nesting += 1;
        let params = self.in_parens(|p| {
            let mut params = Vec::new();
            while !p.at(&TokenKind::RParen) {
                params.push(p.type_expr()?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RParen) {
                return Err(p.error_unexpected("`,` or `)`"));
            }
            Ok((params, p.bump()))
        });
        self.nesting -= 1;
        let (args, close) = params?;
        let mut span = fn_token.span.to(close.span);
        let ret = if self.eat(&TokenKind::Arrow).is_some() {
            let ret = self.type_expr()?;
            span = span.to(ret.span);
            ret
        } else {
            TypeExpr {
                name: Ident {
                    name: "void".into(),
                    span: close.span,
                },
                args: Vec::new(),
                ret: None,
                span: close.span,
            }
        };
        Ok(TypeExpr {
            name: Ident {
                name: "fn".into(),
                span: fn_token.span,
            },
            args,
            ret: Some(Box::new(ret)),
            span,
        })
    }

    /// `if` and `match` as values: parsed as the statements, whose blocks
    /// are newline-sensitive even inside parentheses. Kept out of
    /// `primary`, like the other rarer forms, so that deeply nested
    /// expressions use little stack.
    #[inline(never)]
    fn branch_expr(&mut self) -> PResult<Expr> {
        let saved_parens = std::mem::replace(&mut self.paren_depth, 0);
        let no_struct = std::mem::replace(&mut self.no_struct_literal, false);
        let stmt = if self.at(&TokenKind::If) {
            self.if_stmt()
        } else {
            self.match_stmt()
        };
        self.paren_depth = saved_parens;
        self.no_struct_literal = no_struct;
        let stmt = stmt?;
        Ok(Expr {
            span: stmt.span,
            kind: ExprKind::Branch(Box::new(stmt)),
        })
    }

    /// `(a)`, which is just `a`, or the tuple `(a, b, ...)`.
    #[inline(never)]
    fn paren_or_tuple(&mut self, open: Span) -> PResult<Expr> {
        self.bump();
        let (mut items, close) = self.in_parens(|p| {
            let mut items = vec![p.expr()?];
            while p.eat(&TokenKind::Comma).is_some() {
                if p.at(&TokenKind::RParen) {
                    break;
                }
                items.push(p.expr()?);
            }
            Ok((items, p.expect(&TokenKind::RParen)?))
        })?;
        let span = open.to(close.span);
        let kind = if items.len() == 1 {
            items.pop().expect("one item").kind
        } else {
            ExprKind::Tuple(items)
        };
        Ok(Expr { kind, span })
    }

    /// The rest of an f-string after its `StrHead`: values and text up to
    /// the `StrTail`.
    fn fstring(&mut self, head: String, start: Span) -> PResult<Expr> {
        let mut parts = vec![FStringPart::Text(head)];
        loop {
            let value = self.in_parens(|p| p.expr())?;
            parts.push(FStringPart::Value(value));
            let token = self.peek().clone();
            match token.kind {
                TokenKind::StrMid(text) => {
                    self.bump();
                    parts.push(FStringPart::Text(text));
                }
                TokenKind::StrTail(text) => {
                    self.bump();
                    parts.push(FStringPart::Text(text));
                    parts.retain(|p| !matches!(p, FStringPart::Text(t) if t.is_empty()));
                    return Ok(Expr {
                        kind: ExprKind::FString(parts),
                        span: start.to(token.span),
                    });
                }
                _ => return Err(self.error_unexpected("`}` to end the value in the f-string")),
            }
        }
    }

    /// `fn(x: i64) -> i64 { ... }` in an expression.
    fn lambda(&mut self) -> PResult<Expr> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        if !self.at(&TokenKind::LParen) {
            return Err(self.error_unexpected("`(` after `fn` in an expression"));
        }
        self.bump();
        let params = self.in_parens(|p| {
            let mut params = Vec::new();
            while !p.at(&TokenKind::RParen) {
                params.push(p.param()?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RParen) {
                return Err(p.error_unexpected("`,` or `)`"));
            }
            p.bump();
            Ok(params)
        })?;
        let ret = if self.eat(&TokenKind::Arrow).is_some() {
            Some(self.type_expr()?)
        } else {
            None
        };
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the function body"));
        }
        // The body is a block of statements, even inside parentheses.
        let saved_parens = std::mem::replace(&mut self.paren_depth, 0);
        let body = self.block();
        self.paren_depth = saved_parens;
        let body = body?;
        Ok(Expr {
            span: fn_token.span.to(body.span),
            kind: ExprKind::Lambda { params, ret, body },
        })
    }

    /// `[T, U]` after the name of a generic declaration, if present.
    /// `[T, U]` after the name of a generic struct, enum or `impl`.
    fn type_params(&mut self) -> PResult<Vec<Ident>> {
        let (params, bounds) = self.type_params_with_bounds()?;
        if let Some(bound) = bounds.iter().flatten().next() {
            let diag = Diagnostic::error("trait bounds can only be written on functions")
                .with_code("E0100")
                .with_primary(bound.span, "not allowed here")
                .with_help("write the bound on the functions that need it");
            return Err(self.report(diag));
        }
        Ok(params)
    }

    /// `[T, U: Show + Eq]`: type parameters and the traits each must
    /// implement.
    fn type_params_with_bounds(&mut self) -> PResult<(Vec<Ident>, Vec<Vec<Ident>>)> {
        if !self.at(&TokenKind::LBracket) {
            return Ok((Vec::new(), Vec::new()));
        }
        self.bump();
        self.in_parens(|p| {
            let mut params = Vec::new();
            let mut bounds = Vec::new();
            while !p.at(&TokenKind::RBracket) {
                params.push(p.expect_ident("a type parameter name")?);
                let mut traits = Vec::new();
                if p.eat(&TokenKind::Colon).is_some() {
                    loop {
                        traits.push(p.dotted_ident("a trait name")?);
                        if p.eat(&TokenKind::Plus).is_none() {
                            break;
                        }
                    }
                }
                bounds.push(traits);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RBracket) {
                return Err(p.error_unexpected("`,` or `]`"));
            }
            p.bump();
            Ok((params, bounds))
        })
    }

    /// `name` or `module.name`.
    fn dotted_ident(&mut self, what: &str) -> PResult<Ident> {
        let mut name = self.expect_ident(what)?;
        while self.eat(&TokenKind::Dot).is_some() {
            let part = self.expect_ident(what)?;
            name = Ident {
                name: format!("{}.{}", name.name, part.name),
                span: name.span.to(part.span),
            };
        }
        Ok(name)
    }

    /// `const NAME: type = value`
    fn const_decl(&mut self) -> PResult<ConstDecl> {
        let const_token = self.expect(&TokenKind::Const)?;
        let name = self.expect_ident("a constant name")?;
        let ty = if self.eat(&TokenKind::Colon).is_some() {
            Some(self.type_expr()?)
        } else {
            None
        };
        if !self.at(&TokenKind::Eq) {
            return Err(self.error_unexpected("`=` and the constant's value"));
        }
        self.bump();
        self.skip_newlines();
        let value = self.expr()?;
        Ok(ConstDecl {
            public: false,
            span: const_token.span.to(value.span),
            name,
            ty,
            value,
        })
    }

    /// `trait Name { fn method(self) -> type ... }`
    fn trait_decl(&mut self) -> PResult<TraitDecl> {
        let trait_token = self.expect(&TokenKind::Trait)?;
        let name = self.expect_ident("a trait name")?;
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the trait's methods"));
        }
        self.bump();
        let mut methods = Vec::new();
        let close = loop {
            while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Semi) {
                self.bump();
            }
            match self.peek_kind() {
                TokenKind::RBrace => break self.bump(),
                TokenKind::Fn => {
                    let header = self.fn_header()?;
                    if let Some(first) = header.type_params.first() {
                        let diag = Diagnostic::error("trait methods cannot have type parameters")
                            .with_code("E0100")
                            .with_primary(first.span, "");
                        return Err(self.report(diag));
                    }
                    if self.at(&TokenKind::LBrace) {
                        let diag = Diagnostic::error("trait methods have no body")
                            .with_code("E0100")
                            .with_primary(self.peek().span, "")
                            .with_note("each type that implements the trait writes the body")
                            .with_help("end the method after its signature");
                        return Err(self.report(diag));
                    }
                    methods.push(TraitMethod {
                        span: header.fn_token.span.to(header.end),
                        name: header.name,
                        receiver: header.receiver,
                        params: header.params,
                        ret: header.ret,
                    });
                }
                _ => return Err(self.error_unexpected("`fn` or `}` in the trait")),
            }
        };
        Ok(TraitDecl {
            public: false,
            name,
            methods,
            span: trait_token.span.to(close.span),
        })
    }

    // ---- statements ---------------------------------------------------

    fn block(&mut self) -> PResult<Block> {
        let open = self.expect(&TokenKind::LBrace)?;
        self.enter(0, open.span)?;
        self.nesting += 1;
        let result = self.block_rest(open);
        self.nesting -= 1;
        result
    }

    fn block_rest(&mut self, open: Token) -> PResult<Block> {
        let saved_parens = std::mem::replace(&mut self.paren_depth, 0);
        let mut stmts = Vec::new();
        let close;
        loop {
            while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Semi) {
                self.bump();
            }
            match self.peek_kind() {
                TokenKind::RBrace => {
                    close = self.bump().span;
                    break;
                }
                TokenKind::Eof | TokenKind::Fn => {
                    let found = self.peek().clone();
                    let mut diag = Diagnostic::error(format!(
                        "expected `}}`, found {}",
                        found.kind.describe()
                    ))
                    .with_code("E0100")
                    .with_primary(found.span, "expected `}`")
                    .with_secondary(open.span, "this block is never closed");
                    if found.kind == TokenKind::Fn {
                        diag =
                            diag.with_note("functions cannot be declared inside other functions");
                    }
                    self.report(diag);
                    close = found.span;
                    break;
                }
                _ => {}
            }
            match self.statement() {
                Ok(stmt) => {
                    stmts.push(stmt);
                    if self.check_terminator().is_err() {
                        self.recover_stmt();
                    }
                }
                Err(Reported) => self.recover_stmt(),
            }
        }
        self.paren_depth = saved_parens;
        Ok(Block {
            stmts,
            span: open.span.to(close),
        })
    }

    /// After a statement there must be a newline, `;`, `}` or end of file.
    fn check_terminator(&mut self) -> PResult<()> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Newline | TokenKind::Semi | TokenKind::RBrace | TokenKind::Eof => Ok(()),
            _ => Err(self.error_unexpected("a newline or `;` after the statement")),
        }
    }

    /// One statement. The rarer forms are parsed by separate functions
    /// that are never inlined, so that this one, which recursion through
    /// nested blocks passes on every level, keeps a small stack frame.
    fn statement(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Let => self.let_stmt(),
            TokenKind::If => self.if_stmt(),
            TokenKind::Match => self.match_stmt(),
            TokenKind::For => self.for_stmt(),
            TokenKind::LBrace => self.block_stmt(),
            TokenKind::Return | TokenKind::Break | TokenKind::Continue => self.jump_stmt(),
            TokenKind::While | TokenKind::Loop => self.loop_stmt(),
            TokenKind::Const
            | TokenKind::Else
            | TokenKind::Reserved(_)
            | TokenKind::Impl
            | TokenKind::Struct
            | TokenKind::Enum => Err(self.misplaced_statement()),
            _ => self.expr_stmt(),
        }
    }

    #[inline(never)]
    fn block_stmt(&mut self) -> PResult<Stmt> {
        let block = self.block()?;
        Ok(Stmt {
            span: block.span,
            kind: StmtKind::Block(block),
        })
    }

    /// `return`, `return value`, `break` or `continue`.
    #[inline(never)]
    fn jump_stmt(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let kind = match token.kind {
            TokenKind::Break => StmtKind::Break,
            TokenKind::Continue => StmtKind::Continue,
            _ => {
                let value = match self.peek_kind() {
                    TokenKind::Newline | TokenKind::Semi | TokenKind::RBrace | TokenKind::Eof => {
                        None
                    }
                    _ => Some(self.expr()?),
                };
                let span = value.as_ref().map_or(token.span, |v| token.span.to(v.span));
                return Ok(Stmt {
                    kind: StmtKind::Return(value),
                    span,
                });
            }
        };
        Ok(Stmt {
            kind,
            span: token.span,
        })
    }

    /// `while cond { ... }`, or `loop { ... }`, which is `while true`.
    #[inline(never)]
    fn loop_stmt(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let cond = if token.kind == TokenKind::While {
            self.condition("while")?
        } else {
            self.skip_newlines();
            if !self.at(&TokenKind::LBrace) {
                return Err(self.error_unexpected("`{` after `loop`"));
            }
            Expr {
                kind: ExprKind::Bool(true),
                span: token.span,
            }
        };
        let body = self.block()?;
        Ok(Stmt {
            span: token.span.to(body.span),
            kind: StmtKind::While { cond, body },
        })
    }

    /// An expression, or an assignment to it.
    #[inline(never)]
    fn expr_stmt(&mut self) -> PResult<Stmt> {
        let expr = self.expr()?;
        if let Some(op) = Self::assign_op(self.peek_kind()) {
            return self.assignment(expr, op);
        }
        Ok(Stmt {
            span: expr.span,
            kind: StmtKind::Expr(expr),
        })
    }

    /// Something that cannot start a statement: reports it (skipping a
    /// whole declaration that does not belong in a function).
    #[inline(never)]
    fn misplaced_statement(&mut self) -> Reported {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Reserved(word) => self.reserved_keyword(word, token.span),
            TokenKind::Const => {
                let diag = Diagnostic::error("`const` is only allowed at the top level")
                    .with_code("E0103")
                    .with_primary(token.span, "")
                    .with_help("use `let` inside a function");
                self.report(diag)
            }
            TokenKind::Else => {
                let diag = Diagnostic::error("`else` without a preceding `if`")
                    .with_code("E0100")
                    .with_primary(token.span, "")
                    .with_note("`else` must directly follow the closing `}` of an `if` block");
                self.report(diag)
            }
            TokenKind::Impl => {
                let diag = Diagnostic::error("`impl` blocks cannot be inside functions")
                    .with_code("E0103")
                    .with_primary(token.span, "")
                    .with_help("move the `impl` block to the top level of the file");
                self.report(diag);
                let _ = self.impl_decl();
                Reported
            }
            _ => {
                let is_struct = token.kind == TokenKind::Struct;
                let (what, keyword) = if is_struct {
                    ("structs", "struct")
                } else {
                    ("enums", "enum")
                };
                let diag = Diagnostic::error(format!("{what} cannot be declared inside functions"))
                    .with_code("E0103")
                    .with_primary(token.span, "")
                    .with_help(format!(
                        "move the `{keyword}` declaration to the top level of the file"
                    ));
                self.report(diag);
                // Skip the whole declaration; errors inside it are reported.
                if is_struct {
                    let _ = self.struct_decl();
                } else {
                    let _ = self.enum_decl();
                }
                Reported
            }
        }
    }

    /// `=` gives `Some(None)`, a compound operator such as `+=` gives
    /// `Some(Some(BinaryOp::Add))`.
    fn assign_op(kind: &TokenKind) -> Option<Option<BinaryOp>> {
        Some(match kind {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinaryOp::Add),
            TokenKind::MinusEq => Some(BinaryOp::Sub),
            TokenKind::StarEq => Some(BinaryOp::Mul),
            TokenKind::SlashEq => Some(BinaryOp::Div),
            TokenKind::PercentEq => Some(BinaryOp::Rem),
            _ => return None,
        })
    }

    fn assignment(&mut self, target: Expr, op: Option<BinaryOp>) -> PResult<Stmt> {
        let op_token = self.bump();
        // `a.b[i]` is the variable `a` with the path `[.b, [i]]`.
        let mut path = Vec::new();
        let mut place = &target;
        let target = loop {
            match &place.kind {
                ExprKind::Name(name) => break name.clone(),
                ExprKind::Field { base, field } => {
                    path.push(PlaceElem::Field(field.clone()));
                    place = base;
                }
                ExprKind::Index { base, index } => {
                    path.push(PlaceElem::Index((**index).clone()));
                    place = base;
                }
                _ => {
                    let diag = Diagnostic::error("invalid assignment target")
                        .with_code("E0104")
                        .with_primary(target.span, "cannot assign to this expression")
                        .with_note(
                            "only variables, their fields and list elements can be assigned to",
                        );
                    return Err(self.report(diag));
                }
            }
        };
        path.reverse();
        self.skip_newlines();
        let value = self.expr()?;
        Ok(Stmt {
            span: target.span.to(value.span),
            kind: StmtKind::Assign {
                target,
                path,
                op,
                op_span: op_token.span,
                value,
            },
        })
    }

    /// Parses the condition of an `if` or `while`, which must be followed by
    /// the `{` of its block.
    fn condition(&mut self, keyword: &str) -> PResult<Expr> {
        let no_struct = std::mem::replace(&mut self.no_struct_literal, true);
        let cond = self.expr();
        self.no_struct_literal = no_struct;
        let cond = cond?;
        // As with function bodies, the `{` may start on the next line.
        if *self.peek_past_newlines() == TokenKind::LBrace {
            self.skip_newlines();
        }
        if !self.at(&TokenKind::LBrace) {
            let found = self.peek().clone();
            if found.kind == TokenKind::Eq {
                let diag = Diagnostic::error(format!(
                    "expected `{{` after the `{keyword}` condition, found `=`"
                ))
                .with_code("E0100")
                .with_primary(found.span, "assignment is not allowed here")
                .with_help("use `==` to compare two values");
                return Err(self.report(diag));
            }
            return Err(self.error_unexpected(&format!("`{{` after the `{keyword}` condition")));
        }
        Ok(cond)
    }

    /// The kind of the next token that is not a newline.
    fn peek_past_newlines(&self) -> &TokenKind {
        let mut i = self.current_index();
        while self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        &self.tokens[i].kind
    }

    fn for_stmt(&mut self) -> PResult<Stmt> {
        let for_token = self.bump();
        self.enter(0, for_token.span)?;
        self.nesting += 1;
        let result = self.for_rest(for_token.span);
        self.nesting -= 1;
        result
    }

    fn for_rest(&mut self, start: Span) -> PResult<Stmt> {
        let var = self.expect_ident("a variable name after `for`")?;
        if !self.at(&TokenKind::In) {
            return Err(self.error_unexpected("`in` after the loop variable"));
        }
        self.bump();
        let no_struct = std::mem::replace(&mut self.no_struct_literal, true);
        let first = self.expr();
        let iter = match first {
            Ok(first) if self.eat(&TokenKind::DotDot).is_some() => {
                self.expr().map(|end| ForIter::Range(first, end))
            }
            Ok(first) => Ok(ForIter::Each(first)),
            Err(e) => Err(e),
        };
        self.no_struct_literal = no_struct;
        let iter = iter?;
        if *self.peek_past_newlines() == TokenKind::LBrace {
            self.skip_newlines();
        }
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` after the `for` loop header"));
        }
        let body = self.block()?;
        Ok(Stmt {
            span: start.to(body.span),
            kind: StmtKind::For { var, iter, body },
        })
    }

    fn match_stmt(&mut self) -> PResult<Stmt> {
        let match_token = self.bump();
        self.enter(0, match_token.span)?;
        self.nesting += 1;
        let result = self.match_rest(match_token.span);
        self.nesting -= 1;
        result
    }

    fn match_rest(&mut self, start: Span) -> PResult<Stmt> {
        let scrutinee = self.condition("match")?;
        let open = self.bump();
        let saved_parens = std::mem::replace(&mut self.paren_depth, 0);
        let mut arms = Vec::new();
        let close = loop {
            while matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma
            ) {
                self.bump();
            }
            match self.peek_kind() {
                TokenKind::RBrace => break self.bump().span,
                TokenKind::Eof | TokenKind::Fn => {
                    let found = self.peek().clone();
                    let diag = Diagnostic::error(format!(
                        "expected `}}`, found {}",
                        found.kind.describe()
                    ))
                    .with_code("E0100")
                    .with_primary(found.span, "expected `}`")
                    .with_secondary(open.span, "this `match` is never closed");
                    self.paren_depth = saved_parens;
                    return Err(self.report(diag));
                }
                _ => {}
            }
            match self.match_arm() {
                Ok(arm) => arms.push(arm),
                Err(Reported) => self.recover_stmt(),
            }
        };
        self.paren_depth = saved_parens;
        Ok(Stmt {
            span: start.to(close),
            kind: StmtKind::Match { scrutinee, arms },
        })
    }

    fn match_arm(&mut self) -> PResult<MatchArm> {
        let pattern = self.pattern()?;
        let guard = if self.eat(&TokenKind::If).is_some() {
            Some(self.expr()?)
        } else {
            None
        };
        if !self.at(&TokenKind::FatArrow) {
            let expected = if guard.is_some() {
                "`=>` after the guard"
            } else {
                "`=>` after the pattern"
            };
            return Err(self.error_unexpected(expected));
        }
        self.bump();
        self.skip_newlines();
        let body = if self.at(&TokenKind::LBrace) {
            self.block()?
        } else {
            let stmt = self.statement()?;
            if !matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Semi | TokenKind::Comma | TokenKind::RBrace
            ) {
                return Err(self.error_unexpected("a newline, `,` or `}` after the arm"));
            }
            Block {
                span: stmt.span,
                stmts: vec![stmt],
            }
        };
        Ok(MatchArm {
            span: pattern.span.to(body.span),
            pattern,
            guard,
            body,
        })
    }

    /// A pattern, possibly with `|` between alternatives.
    fn pattern(&mut self) -> PResult<Pattern> {
        let first = self.single_pattern()?;
        if !self.at(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut alternatives = vec![first];
        while self.eat(&TokenKind::Pipe).is_some() {
            alternatives.push(self.single_pattern()?);
        }
        let span = alternatives[0]
            .span
            .to(alternatives.last().expect("at least two").span);
        Ok(Pattern {
            span,
            kind: PatternKind::Or(alternatives),
        })
    }

    fn single_pattern(&mut self) -> PResult<Pattern> {
        let token = self.peek().clone();
        let literal = |kind| Expr {
            kind,
            span: token.span,
        };
        let kind = match &token.kind {
            TokenKind::LParen => {
                self.bump();
                let (items, close) = self.in_parens(|p| {
                    let mut items = Vec::new();
                    while !p.at(&TokenKind::RParen) {
                        items.push(p.pattern()?);
                        if p.eat(&TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                    Ok((items, p.expect(&TokenKind::RParen)?))
                })?;
                let span = token.span.to(close.span);
                if items.len() < 2 {
                    let diag = Diagnostic::error("a tuple pattern has two or more patterns")
                        .with_code("E0100")
                        .with_primary(span, "");
                    return Err(self.report(diag));
                }
                return Ok(Pattern {
                    span,
                    kind: PatternKind::Tuple(items),
                });
            }
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                PatternKind::Wildcard
            }
            TokenKind::Int(v) => {
                self.bump();
                PatternKind::Literal(literal(ExprKind::Int(*v)))
            }
            TokenKind::Str(v) => {
                self.bump();
                PatternKind::Literal(literal(ExprKind::Str(v.clone())))
            }
            TokenKind::True | TokenKind::False => {
                self.bump();
                PatternKind::Literal(literal(ExprKind::Bool(token.kind == TokenKind::True)))
            }
            TokenKind::Minus => {
                let minus = self.bump();
                let int = self.peek().clone();
                let TokenKind::Int(v) = int.kind else {
                    return Err(self.error_unexpected("an integer after `-`"));
                };
                self.bump();
                let span = minus.span.to(int.span);
                let operand = Expr {
                    kind: ExprKind::Int(v),
                    span: int.span,
                };
                return Ok(Pattern {
                    span,
                    kind: PatternKind::Literal(Expr {
                        span,
                        kind: ExprKind::Unary {
                            op: UnaryOp::Neg,
                            op_span: minus.span,
                            operand: Box::new(operand),
                        },
                    }),
                });
            }
            TokenKind::Ident(_) => {
                let first = self.expect_ident("a pattern")?;
                let (enum_name, variant) = if self.eat(&TokenKind::Dot).is_some() {
                    (Some(first), self.expect_ident("a variant name after `.`")?)
                } else {
                    (None, first)
                };
                if enum_name.is_none() && !self.at(&TokenKind::LParen) {
                    // A unit variant or a binding: the checker decides.
                    return Ok(Pattern {
                        span: variant.span,
                        kind: PatternKind::Binding(variant),
                    });
                }
                let mut end = variant.span;
                let mut fields = Vec::new();
                if self.at(&TokenKind::LParen) {
                    self.bump();
                    let close = self.in_parens(|p| {
                        while !p.at(&TokenKind::RParen) {
                            fields.push(p.pattern()?);
                            if p.eat(&TokenKind::Comma).is_none() {
                                break;
                            }
                        }
                        if !p.at(&TokenKind::RParen) {
                            return Err(p.error_unexpected("`,` or `)`"));
                        }
                        Ok(p.bump())
                    })?;
                    end = close.span;
                }
                let start = enum_name.as_ref().map_or(variant.span, |e| e.span);
                return Ok(Pattern {
                    span: start.to(end),
                    kind: PatternKind::Variant {
                        enum_name,
                        variant,
                        fields,
                    },
                });
            }
            TokenKind::Float(_) => {
                self.diags.push(
                    Diagnostic::error("floating-point numbers cannot be matched")
                        .with_code("E0100")
                        .with_primary(token.span, "not a pattern")
                        .with_note("two `f64` values that look equal are often not exactly equal")
                        .with_help("compare with `==`, `<` or `>` in an `if` instead"),
                );
                return Err(Reported);
            }
            _ => return Err(self.error_unexpected("a pattern")),
        };
        Ok(Pattern {
            kind,
            span: token.span,
        })
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let if_token = self.bump();
        self.enter(0, if_token.span)?;
        self.nesting += 1;
        let result = self.if_rest(if_token.span);
        self.nesting -= 1;
        result
    }

    fn if_rest(&mut self, start: Span) -> PResult<Stmt> {
        let cond = self.condition("if")?;
        let then_block = self.block()?;
        let mut end = then_block.span;
        // `else` may follow on the next line.
        let else_branch = if *self.peek_past_newlines() == TokenKind::Else {
            self.skip_newlines();
            self.bump();
            self.skip_newlines();
            let branch = match self.peek_kind() {
                TokenKind::If => self.if_stmt()?,
                TokenKind::LBrace => {
                    let block = self.block()?;
                    Stmt {
                        span: block.span,
                        kind: StmtKind::Block(block),
                    }
                }
                _ => return Err(self.error_unexpected("`{` or `if` after `else`")),
            };
            end = branch.span;
            Some(Box::new(branch))
        } else {
            None
        };
        Ok(Stmt {
            span: start.to(end),
            kind: StmtKind::If {
                cond,
                then_block,
                else_branch,
            },
        })
    }

    fn let_stmt(&mut self) -> PResult<Stmt> {
        let let_token = self.bump();
        let mutable = self.eat(&TokenKind::Mut).is_some();
        if self.at(&TokenKind::LParen) {
            // `let (a, b) = value`
            let pattern = self.single_pattern()?;
            if !self.at(&TokenKind::Eq) {
                return Err(self.error_unexpected("`=` and the value to take apart"));
            }
            self.bump();
            self.skip_newlines();
            let init = self.expr()?;
            return Ok(Stmt {
                span: let_token.span.to(init.span),
                kind: StmtKind::LetPattern {
                    mutable,
                    pattern,
                    init,
                },
            });
        }
        let name = self.expect_ident("a variable name")?;
        let ty = if self.eat(&TokenKind::Colon).is_some() {
            Some(self.type_expr()?)
        } else {
            None
        };
        if !self.at(&TokenKind::Eq) {
            let diag = Diagnostic::error(format!("variable `{}` must be initialised", name.name))
                .with_code("E0100")
                .with_primary(self.peek().span, "expected `=`")
                .with_note("every `let` needs an initial value");
            return Err(self.report(diag));
        }
        self.bump();
        self.skip_newlines();
        let init = self.expr()?;
        Ok(Stmt {
            span: let_token.span.to(init.span),
            kind: StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            },
        })
    }

    // ---- expressions --------------------------------------------------

    fn expr(&mut self) -> PResult<Expr> {
        self.binary(1)
    }

    fn enter(&mut self, extra: usize, span: Span) -> PResult<()> {
        if self.nesting + extra >= MAX_NESTING {
            return Err(self.too_deep(span));
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn too_deep(&mut self, span: Span) -> Reported {
        let diag = Diagnostic::error("code is nested too deeply")
            .with_code("E0105")
            .with_primary(span, "")
            .with_note(format!(
                "blocks and expressions may be nested at most {MAX_NESTING} levels deep"
            ))
            .with_help("split the code using intermediate `let` variables or helper functions");
        self.report(diag)
    }

    fn binary_op(kind: &TokenKind) -> Option<BinaryOp> {
        Some(match kind {
            TokenKind::Plus => BinaryOp::Add,
            TokenKind::Minus => BinaryOp::Sub,
            TokenKind::Star => BinaryOp::Mul,
            TokenKind::Slash => BinaryOp::Div,
            TokenKind::Percent => BinaryOp::Rem,
            TokenKind::EqEq => BinaryOp::Eq,
            TokenKind::BangEq => BinaryOp::Ne,
            TokenKind::Lt => BinaryOp::Lt,
            TokenKind::LtEq => BinaryOp::Le,
            TokenKind::Gt => BinaryOp::Gt,
            TokenKind::GtEq => BinaryOp::Ge,
            TokenKind::AndAnd => BinaryOp::And,
            TokenKind::OrOr => BinaryOp::Or,
            _ => return None,
        })
    }

    /// Precedence climbing: parses operators binding at least as tightly as
    /// `min_prec`. All binary operators are left-associative.
    fn binary(&mut self, min_prec: u8) -> PResult<Expr> {
        self.enter(0, self.peek().span)?;
        self.nesting += 1;
        let result = self.binary_inner(min_prec);
        self.nesting -= 1;
        result
    }

    fn binary_inner(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        // Each loop iteration nests the left operand one level deeper.
        let mut chain = 0;
        let mut last_comparison: Option<(u8, Span)> = None;
        while let Some(op) = Self::binary_op(self.peek_kind()) {
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            let op_token = self.bump();
            if op.is_comparison() {
                if let Some((last_prec, last_span)) = last_comparison
                    && last_prec == prec
                {
                    return Err(self.chained_comparison(op_token.span, last_span));
                }
                last_comparison = Some((prec, op_token.span));
            }
            chain += 1;
            self.enter(chain, op_token.span)?;
            self.skip_newlines();
            self.nesting += chain;
            let rhs = self.binary(prec + 1);
            self.nesting -= chain;
            let rhs = rhs?;
            lhs = Expr {
                span: lhs.span.to(rhs.span),
                kind: ExprKind::Binary {
                    op,
                    op_span: op_token.span,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let op = match self.peek_kind() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            _ => return self.postfix(),
        };
        let op_token = self.bump();
        self.enter(0, op_token.span)?;
        self.nesting += 1;
        let operand = self.unary();
        self.nesting -= 1;
        let operand = operand?;
        Ok(Expr {
            span: op_token.span.to(operand.span),
            kind: ExprKind::Unary {
                op,
                op_span: op_token.span,
                operand: Box::new(operand),
            },
        })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let expr = self.primary()?;
        self.postfix_ops(expr)
    }

    /// The calls, fields, indexing, `?` and struct literals after `expr`.
    /// Separate from `postfix`, whose frame stays small: it is on the
    /// stack once per level of nested parentheses.
    #[inline(never)]
    fn postfix_ops(&mut self, mut expr: Expr) -> PResult<Expr> {
        // Each postfix operation nests the expression one level deeper.
        let mut chain = 0;
        loop {
            // A line that starts with `.` continues the expression:
            //     words
            //         .filter(...)
            if *self.peek_kind() == TokenKind::Newline
                && *self.peek_past_newlines() == TokenKind::Dot
            {
                self.skip_newlines();
            }
            match self.peek_kind() {
                TokenKind::LParen if matches!(expr.kind, ExprKind::Field { .. }) => {
                    let ExprKind::Field { base, field } = expr.kind else {
                        unreachable!("matched above");
                    };
                    self.bump();
                    let (args, close) = self.call_args()?;
                    expr = Expr {
                        span: expr.span.to(close.span),
                        kind: ExprKind::MethodCall {
                            base,
                            name: field,
                            args,
                        },
                    };
                }
                TokenKind::LParen => {
                    self.bump();
                    let (args, close) = self.call_args()?;
                    let span = expr.span.to(close.span);
                    let kind = match expr.kind {
                        ExprKind::Name(callee) => ExprKind::Call { callee, args },
                        // A function value: `make_adder(1)(2)`.
                        kind => ExprKind::CallValue {
                            callee: Box::new(Expr {
                                kind,
                                span: expr.span,
                            }),
                            args,
                        },
                    };
                    expr = Expr { span, kind };
                }
                TokenKind::Dot => {
                    self.bump();
                    // `t.0`: a tuple's field.
                    let field = match self.peek().kind {
                        TokenKind::Int(index) => {
                            let token = self.bump();
                            Ident {
                                name: index.to_string(),
                                span: token.span,
                            }
                        }
                        _ => self.expect_ident("a field name after `.`")?,
                    };
                    expr = Expr {
                        span: expr.span.to(field.span),
                        kind: ExprKind::Field {
                            base: Box::new(expr),
                            field,
                        },
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let (index, close) = self.in_parens(|p| {
                        let index = p.expr()?;
                        Ok((index, p.expect(&TokenKind::RBracket)?))
                    })?;
                    expr = Expr {
                        span: expr.span.to(close.span),
                        kind: ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                    };
                }
                TokenKind::Question => {
                    let question = self.bump();
                    expr = Expr {
                        span: expr.span.to(question.span),
                        kind: ExprKind::Try(Box::new(expr)),
                    };
                }
                TokenKind::LBrace if !self.no_struct_literal => {
                    // `Name { ... }` or `module.Name { ... }`.
                    let Some(name) = dotted_name(&expr) else {
                        break;
                    };
                    expr = self.struct_literal(name)?;
                }
                _ => break,
            }
            chain += 1;
            self.enter(chain, expr.span)?;
        }
        Ok(expr)
    }

    /// After an error inside a struct literal: skips past its closing `}`,
    /// so statement recovery does not mistake it for the end of a block.
    fn skip_to_closing_brace(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.tokens[self.pos].kind {
                TokenKind::Eof | TokenKind::Fn | TokenKind::Struct => return,
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace if depth == 0 => {
                    self.pos += 1;
                    return;
                }
                TokenKind::RBrace => depth -= 1,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// `[a, b]`, `[k: v, ...]` or `[:]`, after the `[`.
    fn collection_literal(&mut self, open: Span) -> PResult<Expr> {
        self.enter(0, open)?;
        self.nesting += 1;
        let result = self.in_parens(|p| {
            if p.at(&TokenKind::Colon) {
                p.bump();
                let close = p.expect(&TokenKind::RBracket)?;
                return Ok(Expr {
                    span: open.to(close.span),
                    kind: ExprKind::MapLit(Vec::new()),
                });
            }
            if let Some(close) = p.eat(&TokenKind::RBracket) {
                return Ok(Expr {
                    span: open.to(close.span),
                    kind: ExprKind::ListLit(Vec::new()),
                });
            }
            let first = p.expr()?;
            if p.eat(&TokenKind::Colon).is_some() {
                let mut entries = vec![(first, p.expr()?)];
                while p.eat(&TokenKind::Comma).is_some() && !p.at(&TokenKind::RBracket) {
                    let key = p.expr()?;
                    p.expect(&TokenKind::Colon)?;
                    entries.push((key, p.expr()?));
                }
                if !p.at(&TokenKind::RBracket) {
                    return Err(p.error_unexpected("`,` or `]`"));
                }
                let close = p.bump();
                return Ok(Expr {
                    span: open.to(close.span),
                    kind: ExprKind::MapLit(entries),
                });
            }
            let mut items = vec![first];
            while p.eat(&TokenKind::Comma).is_some() && !p.at(&TokenKind::RBracket) {
                items.push(p.expr()?);
            }
            if !p.at(&TokenKind::RBracket) {
                return Err(p.error_unexpected("`,` or `]`"));
            }
            let close = p.bump();
            Ok(Expr {
                span: open.to(close.span),
                kind: ExprKind::ListLit(items),
            })
        });
        self.nesting -= 1;
        result
    }

    /// The arguments of a call, after its `(`, and the closing `)`.
    fn call_args(&mut self) -> PResult<(Vec<Expr>, Token)> {
        self.in_parens(|p| {
            let mut args = Vec::new();
            while !p.at(&TokenKind::RParen) {
                args.push(p.expr()?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RParen) {
                return Err(p.error_unexpected("`,` or `)`"));
            }
            Ok((args, p.bump()))
        })
    }

    /// `Name { field: value, ... }`, after the name.
    fn struct_literal(&mut self, name: Ident) -> PResult<Expr> {
        self.bump();
        let result = self.in_parens(|p| {
            let mut fields = Vec::new();
            while !p.at(&TokenKind::RBrace) {
                let field = p.expect_ident("a field name or `}`")?;
                if !p.at(&TokenKind::Colon) {
                    let diag = Diagnostic::error(format!("field `{}` needs a value", field.name))
                        .with_code("E0100")
                        .with_primary(field.span, "expected `:` and a value")
                        .with_help(format!("write `{}: value`", field.name));
                    return Err(p.report(diag));
                }
                p.bump();
                let value = p.expr()?;
                fields.push(FieldInit {
                    span: field.span.to(value.span),
                    name: field,
                    value,
                });
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RBrace) {
                return Err(p.error_unexpected("`,` or `}`"));
            }
            Ok((fields, p.bump()))
        });
        let Ok((fields, close)) = result else {
            self.skip_to_closing_brace();
            return Err(Reported);
        };
        Ok(Expr {
            span: name.span.to(close.span),
            kind: ExprKind::StructLit { name, fields },
        })
    }

    fn primary(&mut self) -> PResult<Expr> {
        let token = self.peek().clone();
        let kind = match token.kind {
            TokenKind::Int(value) => ExprKind::Int(value),
            TokenKind::Float(bits) => ExprKind::Float(bits),
            TokenKind::Str(value) => ExprKind::Str(value),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(name) => ExprKind::Name(Ident {
                name,
                span: token.span,
            }),
            TokenKind::SelfValue => ExprKind::Name(Ident {
                name: "self".into(),
                span: token.span,
            }),
            TokenKind::LBracket => {
                self.bump();
                return self.collection_literal(token.span);
            }
            TokenKind::Fn => return self.lambda(),
            TokenKind::StrHead(text) => {
                self.bump();
                return self.fstring(text, token.span);
            }
            TokenKind::If | TokenKind::Match => return self.branch_expr(),
            TokenKind::LParen => return self.paren_or_tuple(token.span),
            TokenKind::Reserved(word) => return Err(self.reserved_keyword(word, token.span)),
            _ => return Err(self.expected_expression(&token)),
        };
        self.bump();
        Ok(Expr {
            kind,
            span: token.span,
        })
    }
}

#[cfg(test)]
mod tests;
