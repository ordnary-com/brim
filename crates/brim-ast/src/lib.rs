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
    pub enums: Vec<EnumDecl>,
    pub functions: Vec<FnDecl>,
}

/// `struct Name { field: type, ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDecl {
    pub name: Ident,
    pub type_params: Vec<Ident>,
    pub fields: Vec<FieldDecl>,
    /// From `struct` to the closing `}`.
    pub span: Span,
}

/// `enum Name { Variant, Variant(type, ...), ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: Ident,
    pub type_params: Vec<Ident>,
    pub variants: Vec<VariantDecl>,
    /// From `enum` to the closing `}`.
    pub span: Span,
}

/// `Name` or `Name(type, ...)` inside an enum declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantDecl {
    pub name: Ident,
    /// The payload types; empty for a variant without payload.
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

/// One `pattern => body` arm of a `match`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternKind {
    /// `_`: matches anything.
    Wildcard,
    /// `Variant`, `Enum.Variant`, `Variant(a, _)` or `Enum.Variant(a, b)`.
    /// A binding named `_` ignores that payload value.
    Variant {
        enum_name: Option<Ident>,
        variant: Ident,
        bindings: Vec<Ident>,
    },
    /// An integer (possibly negative), string or boolean literal.
    Literal(Expr),
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
    /// `[T, U]` after the name; empty for non-generic functions.
    pub type_params: Vec<Ident>,
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

/// A type as written in source: a name, with type arguments for generic
/// types (`Option[i64]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeExpr {
    pub name: Ident,
    pub args: Vec<TypeExpr>,
    /// From the name to the closing `]`, if any.
    pub span: Span,
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        self.span
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
    /// `match value { pattern => body, ... }`
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
    },
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
    /// `value?`: the value inside an `Ok` or `Some`; an `Err` or `None` is
    /// returned from the function instead.
    Try(Box<Expr>),
    /// `base.name(args)`: an enum variant with a payload (`Shape.Circle(1)`).
    /// Methods will use the same form.
    MethodCall {
        base: Box<Expr>,
        name: Ident,
        args: Vec<Expr>,
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
            ExprKind::Try(inner) => write!(f, "({inner}?)"),
            ExprKind::MethodCall { base, name, args } => {
                write!(f, "{base}.{}(", name.name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str(")")
            }
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
