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
    pub imports: Vec<ImportDecl>,
    pub structs: Vec<StructDecl>,
    pub enums: Vec<EnumDecl>,
    pub impls: Vec<ImplDecl>,
    pub traits: Vec<TraitDecl>,
    pub consts: Vec<ConstDecl>,
    pub functions: Vec<FnDecl>,
}

/// `const NAME: type = value` at the top level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstDecl {
    pub public: bool,
    pub name: Ident,
    /// `None` when the type is inferred from the value.
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    /// From `const` to the end of the value.
    pub span: Span,
}

/// `trait Name { fn method(self, ...) -> type ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraitDecl {
    pub public: bool,
    pub name: Ident,
    pub methods: Vec<TraitMethod>,
    /// From `trait` to the closing `}`.
    pub span: Span,
}

/// A method signature in a trait. `Self` in its types is the type that
/// implements the trait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraitMethod {
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub span: Span,
}

/// `struct Name { field: type, ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDecl {
    pub public: bool,
    pub name: Ident,
    pub type_params: Vec<Ident>,
    pub fields: Vec<FieldDecl>,
    /// From `struct` to the closing `}`.
    pub span: Span,
}

/// `import a.b`, `import a.b as c` or `import a.b.{X, y}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportDecl {
    /// The module path, e.g. `[a, b]`.
    pub path: Vec<Ident>,
    /// `as name`: the name the module is used under.
    pub alias: Option<Ident>,
    /// `.{X, y}`: items brought into scope by their own names.
    pub items: Option<Vec<Ident>>,
    /// `pub import a.{X}`: the items are also items of this module, for
    /// the modules that import it.
    pub public: bool,
    pub span: Span,
}

/// `self` or `mut self`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub mutable: bool,
    pub span: Span,
}

/// `impl Name[T] { fn ... }`: methods of a struct or enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplDecl {
    /// `impl Trait for Type`: the trait.
    pub trait_name: Option<Ident>,
    pub type_name: Ident,
    pub type_params: Vec<Ident>,
    pub methods: Vec<FnDecl>,
    /// From `impl` to the closing `}`.
    pub span: Span,
}

/// `enum Name { Variant, Variant(type, ...), ... }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDecl {
    pub public: bool,
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

/// One `pattern => body` or `pattern if guard => body` arm of a `match`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

/// A piece of an f-string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FStringPart {
    Text(String),
    /// A value shown as text, as `print` shows it.
    Value(Expr),
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
    /// A name that binds the matched value.
    Binding(Ident),
    /// `Variant`, `Enum.Variant`, `Variant(a, _)` or `Enum.Variant(p, q)`:
    /// each payload value is matched by a nested pattern.
    Variant {
        enum_name: Option<Ident>,
        variant: Ident,
        fields: Vec<Pattern>,
    },
    /// An integer (possibly negative), string or boolean literal.
    Literal(Expr),
    /// `p | q`: matches when any alternative matches.
    Or(Vec<Pattern>),
    /// `(p, q, ...)`: a tuple whose values match.
    Tuple(Vec<Pattern>),
}

impl Pattern {
    /// Calls `f` for every name the pattern binds, in source order. In an
    /// or-pattern only the first alternative is visited, since every
    /// alternative binds the same names.
    pub fn for_each_binding(&self, f: &mut impl FnMut(&Ident)) {
        match &self.kind {
            PatternKind::Binding(name) => f(name),
            PatternKind::Variant { fields, .. } => {
                for field in fields {
                    field.for_each_binding(f);
                }
            }
            PatternKind::Or(alternatives) => alternatives[0].for_each_binding(f),
            PatternKind::Tuple(items) => {
                for item in items {
                    item.for_each_binding(f);
                }
            }
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
        }
    }
}

/// `name: type` inside a struct declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDecl {
    /// `pub name: type`: usable from other modules.
    pub public: bool,
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
    /// Declared with `pub`: usable from other modules.
    pub public: bool,
    pub name: Ident,
    /// `[T, U]` after the name; empty for non-generic functions.
    pub type_params: Vec<Ident>,
    /// The traits each type parameter must implement: `[T: Show + Eq]`.
    pub bounds: Vec<Vec<Ident>>,
    /// `self` or `mut self` as the first parameter of a method.
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    /// `None` when the `-> type` clause is omitted (the function returns `void`).
    pub ret: Option<TypeExpr>,
    /// Empty for an `extern fn`.
    pub body: Block,
    /// `extern fn name(...) -> T`: a C function, without a body. The span
    /// of `extern`.
    pub external: Option<Span>,
    /// From `fn` to the closing `}` (to the end of the signature for an
    /// `extern fn`).
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
    /// Type arguments, or the parameter types of a function type.
    pub args: Vec<TypeExpr>,
    /// For a function type `fn(A, B) -> R`, named `fn`: the return type
    /// (`void` when not written).
    pub ret: Option<Box<TypeExpr>>,
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
    /// `let (a, b) = value`: names for the parts of a tuple.
    LetPattern {
        mutable: bool,
        pattern: Pattern,
        init: Expr,
    },
    Let {
        mutable: bool,
        name: Ident,
        /// `None` when the type is inferred from `init`.
        ty: Option<TypeExpr>,
        init: Expr,
    },
    /// `target = value`, or a compound assignment such as `target += value`
    /// (then `op` is `Some(BinaryOp::Add)`). With a non-empty `path`, a field
    /// is assigned: `target.a.b = value` has the path `[a, b]`.
    Assign {
        target: Ident,
        path: Vec<PlaceElem>,
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
    /// `for var in iter { ... }`
    For {
        var: Ident,
        iter: ForIter,
        body: Block,
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

/// One step of an assignment target after the variable: `.field` or
/// `[index]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceElem {
    Field(Ident),
    Index(Expr),
}

/// What a `for` loop iterates over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForIter {
    /// The elements of a list.
    Each(Expr),
    /// `start..end`: the integers from `start` up to, not including, `end`.
    Range(Expr, Expr),
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
    /// A floating-point literal, as the bits of its `f64`.
    Float(u64),
    Bool(bool),
    /// A string literal with escapes already decoded.
    Str(String),
    /// `f"Hello, {name}!"`: text and values, in order.
    FString(Vec<FStringPart>),
    /// `(a, b, ...)`: two or more values.
    Tuple(Vec<Expr>),
    /// A reference to a variable or parameter.
    Name(Ident),
    /// `fn(x: i64) -> i64 { ... }`: a function value, which can use the
    /// variables around it.
    Lambda {
        params: Vec<Param>,
        ret: Option<TypeExpr>,
        body: Block,
    },
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
    /// An `if` with `else`, or a `match`, used as a value: a
    /// [`StmtKind::If`] or [`StmtKind::Match`] whose blocks end with the
    /// value.
    Branch(Box<Stmt>),
    /// A call of a function value that is not a plain name:
    /// `make_adder(1)(2)`.
    CallValue {
        callee: Box<Expr>,
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
    /// `[a, b, c]`
    ListLit(Vec<Expr>),
    /// `[key: value, ...]`, or `[:]` for an empty map.
    MapLit(Vec<(Expr, Expr)>),
    /// `base[index]`
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
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
            ExprKind::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            ExprKind::Bool(v) => write!(f, "{v}"),
            ExprKind::Str(v) => write!(f, "{v:?}"),
            ExprKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(ToString::to_string).collect();
                write!(f, "({})", items.join(", "))
            }
            ExprKind::FString(parts) => {
                f.write_str("f\"")?;
                for part in parts {
                    match part {
                        FStringPart::Text(text) => {
                            f.write_str(&text.replace('{', "{{").replace('}', "}}"))?;
                        }
                        FStringPart::Value(value) => write!(f, "{{{value}}}")?,
                    }
                }
                f.write_str("\"")
            }
            ExprKind::Name(ident) => f.write_str(&ident.name),
            ExprKind::Lambda { params, .. } => {
                let names: Vec<&str> = params.iter().map(|p| p.name.name.as_str()).collect();
                write!(f, "fn({}) {{...}}", names.join(", "))
            }
            ExprKind::Unary { op, operand, .. } => write!(f, "({}{operand})", op.as_str()),
            ExprKind::Binary { op, lhs, rhs, .. } => {
                write!(f, "({lhs} {} {rhs})", op.as_str())
            }
            ExprKind::Branch(stmt) => match &stmt.kind {
                StmtKind::Match { scrutinee, .. } => write!(f, "match {scrutinee} {{...}}"),
                _ => f.write_str("if ... {...} else {...}"),
            },
            ExprKind::CallValue { callee, args } => {
                let args: Vec<String> = args.iter().map(ToString::to_string).collect();
                write!(f, "{callee}({})", args.join(", "))
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
            ExprKind::ListLit(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            ExprKind::MapLit(entries) => {
                if entries.is_empty() {
                    return f.write_str("[:]");
                }
                f.write_str("[")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                f.write_str("]")
            }
            ExprKind::Index { base, index } => write!(f, "{base}[{index}]"),
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
