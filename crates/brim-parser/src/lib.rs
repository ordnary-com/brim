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
    BinaryOp, Block, EnumDecl, Expr, ExprKind, FieldDecl, FieldInit, FnDecl, Ident, MatchArm,
    Param, Pattern, PatternKind, Program, Stmt, StmtKind, StructDecl, TypeExpr, UnaryOp,
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

/// Marker for "an error was already reported". Callers recover and continue.
#[derive(Debug)]
struct Reported;

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
        let mut diag = Diagnostic::error(format!("`{word}` is not supported yet"))
            .with_code("E0101")
            .with_primary(span, "reserved keyword")
            .with_note(format!(
                "`{word}` is reserved for a future version of Brim and cannot be used as a name"
            ));
        if matches!(word, "for" | "loop") {
            diag = diag.with_help("use a `while` loop instead");
        }
        self.report(diag)
    }

    // ---- recovery -----------------------------------------------------

    /// Skips to the start of the next top-level item.
    fn recover_item(&mut self) {
        self.paren_depth = 0;
        self.no_struct_literal = false;
        while !matches!(
            self.peek_kind(),
            TokenKind::Fn | TokenKind::Struct | TokenKind::Enum | TokenKind::Eof
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
        loop {
            while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Semi) {
                self.bump();
            }
            let token = self.peek().clone();
            match &token.kind {
                TokenKind::Eof => break,
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
                        "expected `fn`, `struct` or `enum`, found {}",
                        token.kind.describe()
                    ))
                    .with_code("E0103")
                    .with_primary(token.span, "expected a declaration")
                    .with_note(
                        "only functions, structs and enums can be declared at the top level",
                    );
                    self.report(diag);
                    self.bump();
                    self.recover_item();
                }
            }
        }
        Program {
            structs,
            enums,
            functions,
        }
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
            span: struct_token.span.to(close.span),
            name,
            type_params,
            fields,
        })
    }

    fn function(&mut self) -> PResult<FnDecl> {
        let fn_token = self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident("a function name")?;
        let type_params = self.type_params()?;
        self.expect(&TokenKind::LParen)?;
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
            self.skip_newlines();
            Some(self.type_expr()?)
        } else {
            None
        };
        self.skip_newlines();
        if !self.at(&TokenKind::LBrace) {
            return Err(self.error_unexpected("`{` to start the function body"));
        }
        let body = self.block()?;
        Ok(FnDecl {
            span: fn_token.span.to(body.span),
            name,
            type_params,
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
        let name = self.expect_ident("a type")?;
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
        Ok(TypeExpr { name, args, span })
    }

    /// `[T, U]` after the name of a generic declaration, if present.
    fn type_params(&mut self) -> PResult<Vec<Ident>> {
        if !self.at(&TokenKind::LBracket) {
            return Ok(Vec::new());
        }
        self.bump();
        self.in_parens(|p| {
            let mut params = Vec::new();
            while !p.at(&TokenKind::RBracket) {
                params.push(p.expect_ident("a type parameter name")?);
                if p.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if !p.at(&TokenKind::RBracket) {
                return Err(p.error_unexpected("`,` or `]`"));
            }
            p.bump();
            Ok(params)
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

    fn statement(&mut self) -> PResult<Stmt> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Let => self.let_stmt(),
            TokenKind::Return => {
                self.bump();
                let value = match self.peek_kind() {
                    TokenKind::Newline | TokenKind::Semi | TokenKind::RBrace | TokenKind::Eof => {
                        None
                    }
                    _ => Some(self.expr()?),
                };
                let span = value.as_ref().map_or(token.span, |v| token.span.to(v.span));
                Ok(Stmt {
                    kind: StmtKind::Return(value),
                    span,
                })
            }
            TokenKind::LBrace => {
                let block = self.block()?;
                Ok(Stmt {
                    span: block.span,
                    kind: StmtKind::Block(block),
                })
            }
            TokenKind::If => self.if_stmt(),
            TokenKind::While => {
                self.bump();
                let cond = self.condition("while")?;
                let body = self.block()?;
                Ok(Stmt {
                    span: token.span.to(body.span),
                    kind: StmtKind::While { cond, body },
                })
            }
            TokenKind::Break | TokenKind::Continue => {
                self.bump();
                let kind = if token.kind == TokenKind::Break {
                    StmtKind::Break
                } else {
                    StmtKind::Continue
                };
                Ok(Stmt {
                    kind,
                    span: token.span,
                })
            }
            TokenKind::Else => {
                let diag = Diagnostic::error("`else` without a preceding `if`")
                    .with_code("E0100")
                    .with_primary(token.span, "")
                    .with_note("`else` must directly follow the closing `}` of an `if` block");
                Err(self.report(diag))
            }
            TokenKind::Reserved(word) => Err(self.reserved_keyword(word, token.span)),
            TokenKind::Struct | TokenKind::Enum => {
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
                Err(Reported)
            }
            TokenKind::Match => self.match_stmt(),
            _ => {
                let expr = self.expr()?;
                if let Some(op) = Self::assign_op(self.peek_kind()) {
                    return self.assignment(expr, op);
                }
                Ok(Stmt {
                    span: expr.span,
                    kind: StmtKind::Expr(expr),
                })
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
        // `a.b.c` is the variable `a` with the path `[b, c]`.
        let mut path = Vec::new();
        let mut place = &target;
        let target = loop {
            match &place.kind {
                ExprKind::Name(name) => break name.clone(),
                ExprKind::Field { base, field } => {
                    path.push(field.clone());
                    place = base;
                }
                _ => {
                    let diag = Diagnostic::error("invalid assignment target")
                        .with_code("E0104")
                        .with_primary(target.span, "cannot assign to this expression")
                        .with_note("only variables and their fields can be assigned to");
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
        if !self.at(&TokenKind::FatArrow) {
            return Err(self.error_unexpected("`=>` after the pattern"));
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
            body,
        })
    }

    fn pattern(&mut self) -> PResult<Pattern> {
        let token = self.peek().clone();
        let literal = |kind| Expr {
            kind,
            span: token.span,
        };
        let kind = match &token.kind {
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
                let mut end = variant.span;
                let mut bindings = Vec::new();
                if self.at(&TokenKind::LParen) {
                    self.bump();
                    let close = self.in_parens(|p| {
                        while !p.at(&TokenKind::RParen) {
                            bindings.push(p.expect_ident("a name for the value, or `_`")?);
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
                        bindings,
                    },
                });
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
        let name = self.expect_ident("a variable name")?;
        if !self.at(&TokenKind::Colon) {
            let mut diag =
                Diagnostic::error(format!("variable `{}` needs a type annotation", name.name))
                    .with_code("E0102")
                    .with_primary(name.span, "missing type annotation")
                    .with_help(format!(
                        "write `let {}: TYPE = ...`, for example `let {}: i64 = 0`",
                        name.name, name.name
                    ));
            if self.at(&TokenKind::Eq) {
                diag = diag.with_note("type inference is not supported yet");
            }
            return Err(self.report(diag));
        }
        self.bump();
        let ty = self.type_expr()?;
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
        let mut expr = self.primary()?;
        // Each postfix operation nests the expression one level deeper.
        let mut chain = 0;
        loop {
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
                    let ExprKind::Name(callee) = &expr.kind else {
                        let diag = Diagnostic::error("only named functions can be called")
                            .with_code("E0107")
                            .with_primary(expr.span, "this is not a function name")
                            .with_note("functions are not values yet; call them directly by name");
                        return Err(self.report(diag));
                    };
                    let callee = callee.clone();
                    self.bump();
                    let (args, close) = self.call_args()?;
                    expr = Expr {
                        span: callee.span.to(close.span),
                        kind: ExprKind::Call { callee, args },
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    let field = self.expect_ident("a field name after `.`")?;
                    expr = Expr {
                        span: expr.span.to(field.span),
                        kind: ExprKind::Field {
                            base: Box::new(expr),
                            field,
                        },
                    };
                }
                TokenKind::LBrace if !self.no_struct_literal => {
                    let ExprKind::Name(name) = &expr.kind else {
                        break;
                    };
                    let name = name.clone();
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
            TokenKind::Str(value) => ExprKind::Str(value),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(name) => ExprKind::Name(Ident {
                name,
                span: token.span,
            }),
            TokenKind::LParen => {
                self.bump();
                let (inner, close) = self.in_parens(|p| {
                    let inner = p.expr()?;
                    Ok((inner, p.expect(&TokenKind::RParen)?))
                })?;
                return Ok(Expr {
                    kind: inner.kind,
                    span: token.span.to(close.span),
                });
            }
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
