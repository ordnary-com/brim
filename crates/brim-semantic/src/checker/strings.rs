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
    /// `List[i64]`.
    IntList,
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
        "code_points" => (F(R::StrCodePoints), NONE, Ret::IntList),
        "parse_int" => (I(Intrinsic::ParseInt), NONE, Ret::OptionI64),
        "parse_float" => (I(Intrinsic::ParseFloat), NONE, Ret::OptionF64),
        _ => return None,
    })
}

pub(super) const STR_METHODS: &[&str] = &[
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
    "code_points",
    "char_count",
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
        "from_code_point" => (Intrinsic::FromCodePoint, &[Type::I64], Ret::OptionStr),
        "__fs_list" => (Intrinsic::FsList, STR, Ret::StrList),
        // The second parameter is a `List[string]`, see `check_intrinsic_call`.
        "__process_run" => (Intrinsic::ProcessRun, STR, Ret::I64),
        // `List[u8]` parameters are filled in by `check_intrinsic_call`.
        "__sha256" => (Intrinsic::Sha256, &[], Ret::ByteList),
        "__hmac_sha256" => (Intrinsic::HmacSha256, &[], Ret::ByteList),
        "__md5" => (Intrinsic::Md5, &[], Ret::ByteList),
        "__pbkdf2_sha256" => (Intrinsic::Pbkdf2Sha256, &[], Ret::ByteList),
        "__random_bytes" => (Intrinsic::RandomBytes, &[Type::I64], Ret::ByteList),
        "__net_read_bytes" => (
            Intrinsic::NetReadBytes,
            &[Type::I64, Type::I64],
            Ret::ByteList,
        ),
        "__net_write_bytes" => (Intrinsic::NetWriteBytes, &[], Ret::I64),
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
    "from_code_point",
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
            Ret::IntList => self.list_of(Type::I64),
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
    pub(super) fn fixed_args(
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
        if str_method(&name.name).is_none()
            && let Some(result) = self.prelude_method("string", base, name, args, span, ctx)
        {
            return result;
        }
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

    /// std.json's `__to_json(value)` (a `Json`) and `__from_json(json)` (a
    /// `Result[T, string]`, with `T` from the expected type).
    fn check_json_intrinsic(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> CheckedExpr {
        let json_ty = match self.type_by_name.get("Json") {
            Some(&def @ Def::Enum(_)) => self.types.intern(def, Vec::new()),
            _ => {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is only for std.json", callee.name))
                        .with_code("E0213")
                        .with_primary(callee.span, ""),
                );
                return error_expr(span);
            }
        };
        if callee.name == "__to_json" {
            let checked: Vec<CheckedExpr> = args.iter().map(|a| self.check_expr(a, ctx)).collect();
            let [value] = checked.as_slice() else {
                self.diags.push(
                    Diagnostic::error("`__to_json` takes 1 argument")
                        .with_code("E0205")
                        .with_primary(span, ""),
                );
                return error_expr(span);
            };
            if !self.implements(value.ty, super::traits::DATA) {
                let shown = self.show(value.ty);
                self.diags.push(
                    Diagnostic::error(format!("`{shown}` does not implement `Data`"))
                        .with_code("E0248")
                        .with_primary(value.span, "cannot be turned into JSON"),
                );
                return error_expr(span);
            }
            return CheckedExpr {
                kind: CheckedExprKind::Intrinsic {
                    op: Intrinsic::ToJson,
                    args: checked,
                },
                ty: json_ty,
                span,
            };
        }
        let Some(checked) = self.fixed_args(&callee.name, &[json_ty], args, span, ctx) else {
            return error_expr(span);
        };
        let target = expected.and_then(|t| match t {
            Type::Named(id) => {
                let named = self.types.get(id);
                (named.def == Def::Enum(EnumId(1)) && named.args[1] == Type::String).then_some(t)
            }
            _ => None,
        });
        let Some(ty) = target else {
            self.diags.push(
                Diagnostic::error("`__from_json` needs a `Result[T, string]` to be expected")
                    .with_code("E0206")
                    .with_primary(span, ""),
            );
            return error_expr(span);
        };
        CheckedExpr {
            kind: CheckedExprKind::Intrinsic {
                op: Intrinsic::FromJson,
                args: checked,
            },
            ty,
            span,
        }
    }

    /// `serve_api(module, request)`: an `http.Response` for a call of one of
    /// the module's `pub fn`s (see std.http).
    fn check_serve_api(&mut self, args: &[ast::Expr], span: Span, ctx: &mut FnCtx) -> CheckedExpr {
        let [module_arg, request_arg] = args else {
            for arg in args {
                if self.module_alias(arg, ctx).is_none() {
                    self.check_expr(arg, ctx);
                }
            }
            self.diags.push(
                Diagnostic::error(format!(
                    "`serve_api` takes 2 arguments but {} supplied",
                    supplied(args.len())
                ))
                .with_code("E0205")
                .with_primary(span, "expected a module and an `http.Request`"),
            );
            return error_expr(span);
        };
        let http = self.module_names.iter().position(|n| n == "std.http");
        let http_type = |c: &mut Self, name: &str| {
            let def = *c.scopes[http?].own_types.get(name)?;
            Some(c.types.intern(def, Vec::new()))
        };
        let (Some(request_ty), Some(response_ty)) =
            (http_type(self, "Request"), http_type(self, "Response"))
        else {
            self.diags.push(
                Diagnostic::error("`serve_api` needs `std.http`")
                    .with_code("E0213")
                    .with_primary(span, "")
                    .with_help("add `import std.http`"),
            );
            return error_expr(span);
        };
        let request = self.check_expr_with(request_arg, Some(request_ty), ctx);
        let Some(module) = self.module_alias(module_arg, ctx) else {
            self.check_expr(module_arg, ctx);
            self.diags.push(
                Diagnostic::error("the first argument of `serve_api` must be a module")
                    .with_code("E0206")
                    .with_primary(module_arg.span, "not an imported module")
                    .with_help("import the module with the API functions and pass its name: `serve_api(api, request)`"),
            );
            return error_expr(span);
        };
        if request.ty != request_ty && request.ty != Type::Error {
            let found = self.show(request.ty);
            self.diags.push(
                Diagnostic::error("mismatched types")
                    .with_code("E0206")
                    .with_primary(
                        request.span,
                        format!("expected `http.Request`, found `{found}`"),
                    ),
            );
        }
        if self.api_modules.insert(module) {
            self.check_api_module(module, module_arg.span);
        }
        let module_index = CheckedExpr {
            kind: CheckedExprKind::Int(module as i64),
            ty: Type::I64,
            span: module_arg.span,
        };
        CheckedExpr {
            kind: CheckedExprKind::Intrinsic {
                op: Intrinsic::ServeApi,
                args: vec![module_index, request],
            },
            ty: response_ty,
            span,
        }
    }

    /// Reports the `pub fn`s of an API module whose parameters or result
    /// cannot be sent as JSON.
    fn check_api_module(&mut self, module: usize, used: Span) {
        let module_name = self.module_names[module].clone();
        for id in self.api_candidates(module) {
            let sig = &self.sigs[id.0];
            let name = sig.name.rsplit('.').next().unwrap_or(&sig.name).to_string();
            let name_span = sig.name_span;
            if !sig.type_params.is_empty() {
                self.diags.push(
                    Diagnostic::error(format!("API function `{name}` cannot be generic"))
                        .with_code("E0255")
                        .with_primary(name_span, "")
                        .with_secondary(used, format!("`{module_name}` is served here"))
                        .with_help("make it non-public, or move it out of the API module"),
                );
                continue;
            }
            let params: Vec<(String, Type, Span)> = sig
                .params
                .iter()
                .map(|p| (p.name.clone(), p.ty, p.span))
                .collect();
            let (ret, ret_span) = (sig.ret, sig.ret_span);
            for (param, ty, param_span) in params {
                if !self.implements(ty, super::traits::DATA) {
                    let shown = self.show(ty);
                    self.diags.push(
                        Diagnostic::error(format!(
                            "parameter `{param}` of API function `{name}` cannot be sent as JSON"
                        ))
                        .with_code("E0255")
                        .with_primary(param_span, format!("`{shown}` does not implement `Data`"))
                        .with_secondary(used, format!("`{module_name}` is served here")),
                    );
                }
            }
            if ret != Type::Void && !self.implements(ret, super::traits::DATA) {
                let shown = self.show(ret);
                self.diags.push(
                    Diagnostic::error(format!(
                        "the result of API function `{name}` cannot be sent as JSON"
                    ))
                    .with_code("E0255")
                    .with_primary(
                        ret_span.unwrap_or(name_span),
                        format!("`{shown}` does not implement `Data`"),
                    )
                    .with_secondary(used, format!("`{module_name}` is served here")),
                );
            }
        }
    }

    /// A call of a built-in function with a non-primitive type.
    pub(super) fn check_intrinsic_call(
        &mut self,
        callee: &ast::Ident,
        args: &[ast::Expr],
        span: Span,
        expected: Option<Type>,
        ctx: &mut FnCtx,
    ) -> Option<CheckedExpr> {
        if callee.name == "string_from_bytes" {
            return Some(self.check_string_from_bytes(args, span, ctx));
        }
        if callee.name == "serve_api" {
            return Some(self.check_serve_api(args, span, ctx));
        }
        if brim_stdlib::is_internal(&callee.name) && !self.in_std_module() {
            return None;
        }
        if callee.name == "__to_json" || callee.name == "__from_json" {
            return Some(self.check_json_intrinsic(callee, args, span, expected, ctx));
        }
        let (op, params, ret) = intrinsic_function(&callee.name)?;
        let bytes = self.list_of(Type::U8);
        let params = match op {
            Intrinsic::ProcessRun => vec![Type::String, self.list_of(Type::String)],
            Intrinsic::Sha256 | Intrinsic::Md5 => vec![bytes],
            Intrinsic::HmacSha256 => vec![bytes, bytes],
            Intrinsic::Pbkdf2Sha256 => vec![bytes, bytes, Type::I64, Type::I64],
            Intrinsic::NetWriteBytes => vec![Type::I64, bytes],
            _ => params.to_vec(),
        };
        let Some(checked) = self.fixed_args(&callee.name, &params, args, span, ctx) else {
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
