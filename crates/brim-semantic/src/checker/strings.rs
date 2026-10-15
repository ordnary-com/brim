//! String methods and the built-in functions whose types go beyond the
//! primitive types of `brim-stdlib` (they return lists, `Option` or
//! `Result`).

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};
use brim_runtime::RuntimeFn;

use super::{Checker, FnCtx, error_expr, plural, supplied};
use crate::checked::{CheckedExpr, CheckedExprKind, Intrinsic};
use crate::{Def, EnumId, Type};

/// What a string method does.
enum StrMethod {
    /// A runtime function taking the string and the arguments.
    Runtime(RuntimeFn),
    /// Lowered specially.
    Intrinsic(Intrinsic),
}

/// The result type of a string method or intrinsic.
#[derive(Clone, Copy)]
pub(super) enum Ret {
    I64,
    Bool,
    Str,
    StrList,
    /// `List[u8]`.
    ByteList,
    OptionI64,
    OptionF64,
    OptionStr,
    ResultStr,
    ResultI64,
}

fn str_method(name: &str) -> Option<(StrMethod, &'static [Type], Ret)> {
    use RuntimeFn as R;
    use StrMethod::{Intrinsic as I, Runtime as F};
    const NONE: &[Type] = &[];
    const STR: &[Type] = &[Type::String];
    Some(match name {
        "len" => (F(R::StrLen), NONE, Ret::I64),
        "is_empty" => (I(Intrinsic::StrIsEmpty), NONE, Ret::Bool),
        "contains" => (F(R::StrContains), STR, Ret::Bool),
        "starts_with" => (F(R::StrStartsWith), STR, Ret::Bool),
        "ends_with" => (F(R::StrEndsWith), STR, Ret::Bool),
        "find" => (I(Intrinsic::StrFind), STR, Ret::OptionI64),
        "slice" => (F(R::StrSlice), &[Type::I64, Type::I64], Ret::Str),
        "trim" => (F(R::StrTrim), NONE, Ret::Str),
        "trim_start" => (F(R::StrTrimStart), NONE, Ret::Str),
        "trim_end" => (F(R::StrTrimEnd), NONE, Ret::Str),
        "to_upper" => (F(R::StrToUpper), NONE, Ret::Str),
        "to_lower" => (F(R::StrToLower), NONE, Ret::Str),
        "replace" => (F(R::StrReplace), &[Type::String, Type::String], Ret::Str),
        "repeat" => (F(R::StrRepeat), &[Type::I64], Ret::Str),
        "split" => (F(R::StrSplit), STR, Ret::StrList),
        "lines" => (F(R::StrLines), NONE, Ret::StrList),
        "chars" => (F(R::StrChars), NONE, Ret::StrList),
        "bytes" => (F(R::StrBytes), NONE, Ret::ByteList),
        "parse_int" => (I(Intrinsic::ParseInt), NONE, Ret::OptionI64),
        "parse_float" => (I(Intrinsic::ParseFloat), NONE, Ret::OptionF64),
        _ => return None,
    })
}

const STR_METHODS: &[&str] = &[
    "len",
    "is_empty",
    "contains",
    "starts_with",
    "ends_with",
    "find",
    "slice",
    "trim",
    "trim_start",
    "trim_end",
    "to_upper",
    "to_lower",
    "replace",
    "repeat",
    "split",
    "lines",
    "chars",
    "bytes",
    "parse_int",
    "parse_float",
];

/// Built-in functions with non-primitive types.
pub(super) fn intrinsic_function(name: &str) -> Option<(Intrinsic, &'static [Type], Ret)> {
    const STR: &[Type] = &[Type::String];
    Some(match name {
        "read_line" => (Intrinsic::ReadLine, &[], Ret::OptionStr),
        "read_file" => (Intrinsic::ReadFile, STR, Ret::ResultStr),
        "write_file" => (
            Intrinsic::WriteFile,
            &[Type::String, Type::String],
            Ret::ResultI64,
        ),
        "args" => (Intrinsic::Args, &[], Ret::StrList),
        "env" => (Intrinsic::Env, STR, Ret::OptionStr),
        _ => return None,
    })
}

pub(super) const INTRINSIC_FUNCTIONS: &[&str] = &[
    "read_line",
    "read_file",
    "write_file",
    "args",
    "env",
    "string_from_bytes",
];

impl<'a> Checker<'a> {
    fn ret_type(&mut self, ret: Ret) -> Type {
        let option = |c: &mut Self, t| c.types.intern(Def::Enum(EnumId(0)), vec![t]);
        let result = |c: &mut Self, t| c.types.intern(Def::Enum(EnumId(1)), vec![t, Type::String]);
        match ret {
            Ret::I64 => Type::I64,
            Ret::Bool => Type::Bool,
            Ret::Str => Type::String,
            Ret::StrList => self.list_of(Type::String),
            Ret::ByteList => self.list_of(Type::U8),
            Ret::OptionI64 => option(self, Type::I64),
            Ret::OptionF64 => option(self, Type::F64),
            Ret::OptionStr => option(self, Type::String),
            Ret::ResultStr => result(self, Type::String),
            Ret::ResultI64 => result(self, Type::I64),
        }
    }

    /// `string_from_bytes(bytes)`: `Some` text if the bytes are UTF-8.
    fn check_string_from_bytes(
        &mut self,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let bytes = self.list_of(Type::U8);
        let checked: Vec<CheckedExpr> = args
            .iter()
            .map(|a| self.check_expr_with(a, Some(bytes), ctx))
            .collect();
        if checked.len() != 1 {
            self.diags.push(
                Diagnostic::error(format!(
                    "`string_from_bytes` takes 1 argument but {} supplied",
                    supplied(checked.len())
                ))
                .with_code("E0205")
                .with_primary(span, "expected 1 argument"),
            );
            return error_expr(span);
        }
        let arg = &checked[0];
        if arg.ty == Type::Error {
            return error_expr(span);
        }
        if arg.ty != bytes {
            let found = self.show(arg.ty);
            self.diags.push(
                Diagnostic::error("mismatched types")
                    .with_code("E0206")
                    .with_primary(arg.span, format!("expected `List[u8]`, found `{found}`"))
                    .with_note("`string_from_bytes` reads UTF-8 bytes"),
            );
            return error_expr(span);
        }
        CheckedExpr {
            ty: self.ret_type(Ret::OptionStr),
            kind: CheckedExprKind::Intrinsic {
                op: Intrinsic::StrFromBytes,
                args: checked,
            },
            span,
        }
    }

    /// Checks arguments against fixed parameter types.
    fn fixed_args(
        &mut self,
        what: &str,
        params: &[Type],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<Vec<CheckedExpr>> {
        if args.len() != params.len() {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "`{what}` takes {} but {} supplied",
                    plural(params.len(), "argument"),
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(
                    span,
                    format!("expected {}", plural(params.len(), "argument")),
                ),
            );
            return None;
        }
        let mut checked = Vec::with_capacity(args.len());
        for (arg, param) in args.iter().zip(params) {
            let arg = self.check_expr_with(arg, Some(*param), ctx);
            self.expect_type(&arg, *param, None, "");
            checked.push(arg);
        }
        Some(checked)
    }

    /// `text.name(args)` on a string.
    pub(super) fn check_str_method(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let receiver = self.check_expr(base, ctx);
        let Some((method, params, ret)) = str_method(&name.name) else {
            for arg in args {
                self.check_expr(arg, ctx);
            }
            let mut diag = Diagnostic::error(format!("`string` has no method `{}`", name.name))
                .with_code("E0231")
                .with_primary(name.span, "unknown method");
            diag = match crate::suggest::similar(&name.name, STR_METHODS.iter().copied()) {
                Some(similar) => diag.with_help(format!("did you mean `{similar}`?")),
                None => diag.with_note(format!(
                    "string methods: {}",
                    STR_METHODS
                        .iter()
                        .map(|m| format!("`{m}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
            self.diags.push(diag);
            return error_expr(span);
        };
        let Some(checked) = self.fixed_args(&name.name, params, args, span, ctx) else {
            return error_expr(span);
        };
        let mut all = Vec::with_capacity(checked.len() + 1);
        all.push(receiver);
        all.extend(checked);
        let ty = self.ret_type(ret);
        let kind = match method {
            StrMethod::Runtime(runtime) => CheckedExprKind::Builtin { runtime, args: all },
            StrMethod::Intrinsic(op) => CheckedExprKind::Intrinsic { op, args: all },
        };
        CheckedExpr { kind, ty, span }
    }

    /// A call of a built-in function with a non-primitive type.
    pub(super) fn check_intrinsic_call(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        if callee.name == "string_from_bytes" {
            return Some(self.check_string_from_bytes(args, span, ctx));
        }
        let (op, params, ret) = intrinsic_function(&callee.name)?;
        let Some(checked) = self.fixed_args(&callee.name, params, args, span, ctx) else {
            return Some(error_expr(span));
        };
        let ty = self.ret_type(ret);
        Some(CheckedExpr {
            kind: CheckedExprKind::Intrinsic { op, args: checked },
            ty,
            span,
        })
    }
}
