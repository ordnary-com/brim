//! The Brim abstract syntax tree.
//!
//! The AST is a faithful, untyped representation of the source program as
//! written. It carries [`Span`]s for diagnostics but no type or name-resolution
//! information; that is produced by `brim-semantic`.
//!
//! Every expression implements [`std::fmt::Display`] as a fully parenthesised
//! form (e.g. `(1 + (2 * 3))`), which makes precedence easy to test.

use std::fmt;

pub use brim_diagnostics::Span;

/// A whole source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub structs: Vec<StructDecl>,
    pub functions: Vec<FnDecl>,
}

/// `struct Name { field: type, ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    /// From `struct` to the closing `}`.
    pub span: Span,
}

/// `name: type` inside a struct declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// `name: value` inside a struct literal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// A name together with where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// `fn name(params) -> ret { body }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    /// `None` when the `-> type` clause is omitted (the function returns `void`).
    pub ret: Option<TypeExpr>,
    pub body: Block,
    /// From `fn` to the closing `}`.
    pub span: Span,
}

impl FnDecl {
    /// The span of the signature: from `fn` to the end of the return type.
    pub fn signature_span(&self) -> Span {
        let end = self.ret.as_ref().map_or_else(
            || self.params.last().map_or(self.name.span, |p| p.span),
            |r| r.span(),
        );
        Span::new(self.span.start, end.end)
    }
}

/// `name: type`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// A type as written in source. Only named types exist so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeExpr {
    pub name: Ident,
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        self.name.span
    }
}

/// `{ stmt* }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// From `{` to `}` inclusive.
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    /// `let name: type = init` or `let mut name: type = init`
    Let {
        mutable: bool,
        name: Ident,
        ty: TypeExpr,
        init: Expr,
    },
    /// `target = value`, or a compound assignment such as `target += value`
    /// (then `op` is `Some(BinaryOp::Add)`). With a non-empty `path`, a field
    /// is assigned: `target.a.b = value` has the path `[a, b]`.
    Assign {
        target: Ident,
        path: Vec<Ident>,
        op: Option<BinaryOp>,
        /// The span of `=` or of the compound operator.
        op_span: Span,
        value: Expr,
    },
    /// `if cond { ... } else ...`
    If {
        cond: Expr,
        then_block: Block,
        /// Either a [`StmtKind::Block`] (`else { ... }`) or another
        /// [`StmtKind::If`] (`else if ...`).
        else_branch: Option<Box<Stmt>>,
    },
    /// `while cond { ... }`
    While { cond: Expr, body: Block },
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `return` or `return expr`
    Return(Option<Expr>),
    /// An expression evaluated for its effect, e.g. a call.
    Expr(Expr),
    /// A nested block, which introduces a new scope.
    Block(Block),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    /// A decimal integer literal. Stored unsigned; range checking happens in
    /// semantic analysis.
    Int(u64),
    Bool(bool),
    /// A string literal with escapes already decoded.
    Str(String),
    /// A reference to a variable or parameter.
    Name(Ident),
    Unary {
        op: UnaryOp,
        op_span: Span,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        op_span: Span,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `callee(args)`. Only named functions can be called.
    Call {
        callee: Ident,
        args: Vec<Expr>,
    },
    /// `Name { field: value, ... }`
    StructLit {
        name: Ident,
        fields: Vec<FieldInit>,
    },
    /// `base.field`
    Field {
        base: Box<Expr>,
        field: Ident,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `!x`
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
        }
    }

    /// Binding strength: higher binds tighter. See the precedence table in
    /// `docs/language-spec.md`.
    pub fn precedence(self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq | BinaryOp::Ne => 3,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 4,
            BinaryOp::Add | BinaryOp::Sub => 5,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 6,
        }
    }

    /// Comparison operators cannot be chained (`a < b < c` is an error).
    pub fn is_comparison(self) -> bool {
        matches!(self.precedence(), 3 | 4)
    }
}

impl UnaryOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Int(v) => write!(f, "{v}"),
            ExprKind::Bool(v) => write!(f, "{v}"),
            ExprKind::Str(v) => write!(f, "{v:?}"),
            ExprKind::Name(ident) => f.write_str(&ident.name),
            ExprKind::Unary { op, operand, .. } => write!(f, "({}{operand})", op.as_str()),
            ExprKind::Binary { op, lhs, rhs, .. } => {
                write!(f, "({lhs} {} {rhs})", op.as_str())
            }
            ExprKind::Call { callee, args } => {
                write!(f, "{}(", callee.name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str(")")
            }
            ExprKind::StructLit { name, fields } => {
                write!(f, "{} {{", name.name)?;
                for (i, field) in fields.iter().enumerate() {
                    let sep = if i > 0 { "," } else { "" };
                    write!(f, "{sep} {}: {}", field.name.name, field.value)?;
                }
                f.write_str(if fields.is_empty() { "}" } else { " }" })
            }
            ExprKind::Field { base, field } => write!(f, "({base}.{})", field.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(kind: ExprKind) -> Expr {
        Expr {
            kind,
            span: Span::default(),
        }
    }

    #[test]
    fn display_is_fully_parenthesised() {
        let e = expr(ExprKind::Binary {
            op: BinaryOp::Add,
            op_span: Span::default(),
            lhs: Box::new(expr(ExprKind::Int(1))),
            rhs: Box::new(expr(ExprKind::Unary {
                op: UnaryOp::Neg,
                op_span: Span::default(),
                operand: Box::new(expr(ExprKind::Call {
                    callee: Ident {
                        name: "f".into(),
                        span: Span::default(),
                    },
                    args: vec![expr(ExprKind::Str("a".into())), expr(ExprKind::Bool(true))],
                })),
            })),
        });
        assert_eq!(e.to_string(), "(1 + (-f(\"a\", true)))");
    }

    #[test]
    fn precedence_ordering() {
        assert!(BinaryOp::Mul.precedence() > BinaryOp::Add.precedence());
        assert!(BinaryOp::Add.precedence() > BinaryOp::Lt.precedence());
        assert!(BinaryOp::Lt.precedence() > BinaryOp::Eq.precedence());
        assert!(BinaryOp::Eq.precedence() > BinaryOp::And.precedence());
        assert!(BinaryOp::And.precedence() > BinaryOp::Or.precedence());
        assert!(BinaryOp::Ge.is_comparison() && !BinaryOp::And.is_comparison());
    }
}
