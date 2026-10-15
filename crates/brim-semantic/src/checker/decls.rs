//! Declarations: the built-in enums, structs, enums and function signatures.

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span};

use super::{Checker, EnumInfo, FieldInfo, ParamSig, Signature, StructInfo, VariantInfo};
use crate::checked::FnId;
use crate::suggest;
use crate::{Def, EnumId, StructId, Type};

/// Names that cannot be used for a struct, enum or type parameter.
fn reserved_type_name(name: &str) -> bool {
    Type::from_name(name).is_some() || matches!(name, "Option" | "Result" | "List" | "Map" | "Box")
}

impl<'a> Checker<'a> {
    /// `enum Option[T] { Some(T), None }` and
    /// `enum Result[T, E] { Ok(T), Err(E) }`, available in every program,
    /// with their variants usable without the enum's name.
    pub(super) fn add_prelude(&mut self) {
        let variant = |name: &str, fields: Vec<Type>| VariantInfo {
            name: name.to_string(),
            fields,
            span: None,
        };
        self.enums.push(EnumInfo {
            name: "Option".into(),
            name_span: None,
            type_params: vec!["T".into()],
            variants: vec![
                variant("Some", vec![Type::Param(0)]),
                variant("None", vec![]),
            ],
            module: usize::MAX,
            public: true,
        });
        self.enums.push(EnumInfo {
            name: "Result".into(),
            name_span: None,
            type_params: vec!["T".into(), "E".into()],
            variants: vec![
                variant("Ok", vec![Type::Param(0)]),
                variant("Err", vec![Type::Param(1)]),
            ],
            module: usize::MAX,
            public: true,
        });
        for (name, e, v) in [("Some", 0, 0), ("None", 0, 1), ("Ok", 1, 0), ("Err", 1, 1)] {
            self.prelude_variants.insert(name, (EnumId(e), v));
        }
    }

    /// Declares a module's struct and enum names, so every module's fields
    /// and signatures can refer to them. Returns where the module's structs
    /// and enums start in the global tables.
    pub(super) fn declare_types(&mut self, program: &'a ast::Program) -> (usize, usize) {
        let struct_base = self.structs.len();
        let enum_base = self.enums.len();
        for (i, decl) in program.structs.iter().enumerate() {
            self.declare_type_name(&decl.name, Def::Struct(StructId(struct_base + i)));
            let type_params = self.type_param_names(&decl.type_params);
            self.structs.push(StructInfo {
                name: decl.name.name.clone(),
                name_span: decl.name.span,
                span: decl.span,
                type_params,
                fields: Vec::new(),
                module: self.module,
                public: decl.public,
            });
        }
        for (i, decl) in program.enums.iter().enumerate() {
            self.declare_type_name(&decl.name, Def::Enum(EnumId(enum_base + i)));
            let type_params = self.type_param_names(&decl.type_params);
            self.enums.push(EnumInfo {
                name: decl.name.name.clone(),
                name_span: Some(decl.name.span),
                type_params,
                variants: Vec::new(),
                module: self.module,
                public: decl.public,
            });
        }
        (struct_base, enum_base)
    }

    /// Resolves the fields and variants of a module's types.
    pub(super) fn define_types(
        &mut self,
        program: &'a ast::Program,
        struct_base: usize,
        enum_base: usize,
    ) {
        for (i, decl) in program.structs.iter().enumerate() {
            let i = struct_base + i;
            self.type_params = self.structs[i].type_params.clone();
            let mut fields: Vec<FieldInfo> = Vec::new();
            for field in &decl.fields {
                if let Some(previous) = fields.iter().find(|f| f.name == field.name.name) {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "field `{}` is declared more than once",
                            field.name.name
                        ))
                        .with_code("E0220")
                        .with_primary(field.name.span, "duplicate field")
                        .with_secondary(previous.span, "first declared here"),
                    );
                }
                let ty = self.resolve_type(&field.ty, "a field");
                fields.push(FieldInfo {
                    name: field.name.name.clone(),
                    ty,
                    span: field.span,
                });
            }
            self.structs[i].fields = fields;
        }
        for (i, decl) in program.enums.iter().enumerate() {
            let id = enum_base + i;
            self.type_params = self.enums[id].type_params.clone();
            let mut variants: Vec<VariantInfo> = Vec::new();
            for variant in &decl.variants {
                if let Some(previous) = variants.iter().find(|v| v.name == variant.name.name) {
                    let mut diag = Diagnostic::error(format!(
                        "variant `{}` is declared more than once",
                        variant.name.name
                    ))
                    .with_code("E0220")
                    .with_primary(variant.name.span, "duplicate variant");
                    if let Some(span) = previous.span {
                        diag = diag.with_secondary(span, "first declared here");
                    }
                    self.diags.push(diag);
                }
                let fields = variant
                    .fields
                    .iter()
                    .map(|t| self.resolve_type(t, "a variant's value"))
                    .collect();
                variants.push(VariantInfo {
                    name: variant.name.name.clone(),
                    fields,
                    span: Some(variant.span),
                });
            }
            self.enums[id].variants = variants;
        }
        self.type_params.clear();
    }

    fn declare_type_name(&mut self, name: &ast::Ident, def: Def) {
        let text = name.name.as_str();
        if reserved_type_name(text) {
            self.diags.push(
                Diagnostic::error(format!("`{text}` is a built-in type"))
                    .with_code("E0219")
                    .with_primary(name.span, "cannot be used as a type name")
                    .with_help("choose a different name"),
            );
        } else if let Some(first) = self.type_by_name.get(text) {
            let mut diag = Diagnostic::error(format!("type `{text}` is defined more than once"))
                .with_code("E0219")
                .with_primary(name.span, "redefined here");
            if let Some(span) = self.def_span(*first) {
                diag = diag.with_secondary(span, "first defined here");
            }
            self.diags.push(diag);
        } else {
            self.type_by_name.insert(text.to_string(), def);
        }
    }

    /// The span of a declaration's name, if it has one in the source.
    pub(super) fn def_span(&self, def: Def) -> Option<Span> {
        match def {
            Def::Struct(id) => Some(self.structs[id.0].name_span),
            Def::Enum(id) => self.enums[id.0].name_span,
            Def::List | Def::Map | Def::Box | Def::Fn => None,
        }
    }

    /// Checks the type parameter list of a declaration.
    fn type_param_names(&mut self, params: &[ast::Ident]) -> Vec<String> {
        let mut names: Vec<String> = Vec::with_capacity(params.len());
        for (i, param) in params.iter().enumerate() {
            if let Some(first) = params[..i].iter().find(|p| p.name == param.name) {
                self.diags.push(
                    Diagnostic::error(format!(
                        "type parameter `{}` is declared more than once",
                        param.name
                    ))
                    .with_code("E0220")
                    .with_primary(param.span, "duplicate type parameter")
                    .with_secondary(first.span, "first declared here"),
                );
            } else if reserved_type_name(&param.name) {
                self.diags.push(
                    Diagnostic::error(format!("`{}` is a built-in type", param.name))
                        .with_code("E0219")
                        .with_primary(param.span, "cannot be used as a type parameter")
                        .with_help("type parameters are usually single capital letters: `T`, `U`"),
                );
            }
            names.push(param.name.clone());
        }
        names
    }

    /// A struct or enum that contains itself, directly or through other
    /// types, would be infinitely large.
    pub(super) fn check_recursive_types(&mut self) {
        // Nodes: structs, then enums. Edges: (member label, span, target).
        let struct_count = self.structs.len();
        let node = |def: Def| match def {
            Def::Struct(s) => s.0,
            Def::Enum(e) => struct_count + e.0,
            Def::List | Def::Map | Def::Box | Def::Fn => {
                unreachable!("collections and boxes keep their contents on the heap")
            }
        };
        let total = struct_count + self.enums.len();
        let mut edges: Vec<Vec<(String, Option<Span>, usize)>> = vec![Vec::new(); total];
        let mut mentioned = Vec::new();
        for (s, info) in self.structs.iter().enumerate() {
            for field in &info.fields {
                mentioned.clear();
                self.types.defs_in(field.ty, &mut mentioned);
                for def in &mentioned {
                    let label = format!("{}.{}", info.name, field.name);
                    edges[s].push((label, Some(field.span), node(*def)));
                }
            }
        }
        for (e, info) in self.enums.iter().enumerate() {
            for variant in &info.variants {
                for ty in &variant.fields {
                    mentioned.clear();
                    self.types.defs_in(*ty, &mut mentioned);
                    for def in &mentioned {
                        let label = format!("{}.{}", info.name, variant.name);
                        edges[struct_count + e].push((label, variant.span, node(*def)));
                    }
                }
            }
        }

        let mut reported = vec![false; total];
        for start in 0..total {
            if reported[start] {
                continue;
            }
            let mut path: Vec<(usize, usize)> = Vec::new(); // (node, edge)
            let mut visited = vec![false; total];
            if !find_cycle(&edges, start, start, &mut path, &mut visited) {
                continue;
            }
            for &(n, _) in &path {
                reported[n] = true;
            }
            let chain: Vec<&str> = path.iter().map(|&(n, e)| edges[n][e].0.as_str()).collect();
            let (is_struct, name, name_span) = if start < struct_count {
                let info = &self.structs[start];
                (true, info.name.clone(), Some(info.name_span))
            } else {
                let info = &self.enums[start - struct_count];
                (false, info.name.clone(), info.name_span)
            };
            let (first_node, first_edge) = path[0];
            let mut diag = Diagnostic::error(format!(
                "{} `{name}` contains itself",
                if is_struct { "struct" } else { "enum" }
            ))
            .with_code("E0221");
            if let Some(span) = name_span {
                diag = diag.with_primary(span, "infinitely large");
            }
            if let Some(span) = edges[first_node][first_edge].1 {
                diag = diag.with_secondary(span, format!("`{name}` contains itself through here"));
            }
            diag = diag
                .with_note(format!("the cycle is: {} -> {name}", chain.join(" -> ")))
                .with_note("structs and enums are values: what they contain is stored inside them")
                .with_help(format!(
                    "store it in a `Box[{name}]`, which keeps the value on the heap"
                ));
            self.diags.push(diag);
        }
    }

    /// Declares a module's function names.
    pub(super) fn declare_functions(&mut self, program: &'a ast::Program) {
        for f in &program.functions {
            let id = FnId(self.fn_decls.len());
            let name = f.name.name.as_str();
            if brim_stdlib::lookup(name).is_some()
                || self.prelude_variants.contains_key(name)
                || super::strings::intrinsic_function(name).is_some()
            {
                self.diags.push(
                    Diagnostic::error(format!("cannot redefine builtin function `{name}`"))
                        .with_code("E0216")
                        .with_primary(f.name.span, "this name belongs to a builtin")
                        .with_help("choose a different name"),
                );
            } else if let Some(def) = self.type_by_name.get(name).copied() {
                let kind = match def {
                    Def::Struct(_) => "struct",
                    Def::Enum(_) => "enum",
                    Def::List | Def::Map | Def::Box | Def::Fn => "built-in type",
                };
                let mut diag =
                    Diagnostic::error(format!("`{name}` is already the name of a {kind}"))
                        .with_code("E0219")
                        .with_primary(f.name.span, "function declared here")
                        .with_help("functions and types need different names");
                if let Some(span) = self.def_span(def) {
                    diag = diag.with_secondary(span, format!("{kind} declared here"));
                }
                self.diags.push(diag);
            } else if let Some(first) = self.by_name.get(name) {
                let first_span = self.fn_decls[first.0].name.span;
                self.diags.push(
                    Diagnostic::error(format!("function `{name}` is defined more than once"))
                        .with_code("E0202")
                        .with_primary(f.name.span, "redefined here")
                        .with_secondary(first_span, "first defined here"),
                );
            } else {
                self.by_name.insert(name, id);
            }
            self.fn_decls.push(f);
        }
    }

    /// Resolves the signatures of a module's functions (declared before).
    pub(super) fn collect_functions(&mut self, program: &'a ast::Program) {
        for f in &program.functions {
            if let Some(receiver) = &f.receiver {
                self.diags.push(
                    Diagnostic::error("`self` is only allowed in methods")
                        .with_code("E0239")
                        .with_primary(receiver.span, "")
                        .with_help("declare the function inside an `impl` block"),
                );
            }
            let sig = self.signature(f, Vec::new(), None);
            self.sigs.push(sig);
        }
    }

    /// Methods from `impl` blocks. They become functions after the free
    /// functions, with `self` (if any) as their first parameter.
    pub(super) fn collect_impls(&mut self, program: &'a ast::Program) {
        for decl in &program.impls {
            let name = decl.type_name.name.as_str();
            let def = match self.type_by_name.get(name).copied() {
                Some(def @ (Def::Struct(_) | Def::Enum(_))) if self.def_span(def).is_some() => def,
                Some(_) => {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "cannot add methods to the built-in type `{name}`"
                        ))
                        .with_code("E0237")
                        .with_primary(decl.type_name.span, "")
                        .with_help("write a function that takes the value as a parameter"),
                    );
                    continue;
                }
                None => {
                    self.unknown_type(name, decl.type_name.span);
                    continue;
                }
            };
            if self.def_module(def) != self.module {
                self.diags.push(
                    Diagnostic::error(format!(
                        "methods of `{name}` must be declared in the module that declares it"
                    ))
                    .with_code("E0237")
                    .with_primary(decl.type_name.span, ""),
                );
                continue;
            }
            let impl_params = self.type_param_names(&decl.type_params);
            let expected = self.def_type_params(def).len();
            if impl_params.len() != expected {
                let params = self.def_type_params(def).join(", ");
                let written = if expected == 0 {
                    format!("impl {name}")
                } else {
                    format!("impl {name}[{params}]")
                };
                self.diags.push(
                    Diagnostic::error(format!(
                        "`{name}` takes {} but the `impl` has {}",
                        super::plural(expected, "type parameter"),
                        impl_params.len()
                    ))
                    .with_code("E0225")
                    .with_primary(decl.type_name.span, "")
                    .with_help(format!("write `{written}`")),
                );
                continue;
            }
            for method in &decl.methods {
                let key = (def, method.name.name.clone());
                if let Some(first) = self.methods.get(&key) {
                    let first_span = self.sigs[first.0].name_span;
                    self.diags.push(
                        Diagnostic::error(format!(
                            "method `{}` is defined more than once for `{name}`",
                            method.name.name
                        ))
                        .with_code("E0238")
                        .with_primary(method.name.span, "redefined here")
                        .with_secondary(first_span, "first defined here"),
                    );
                    continue;
                }
                if let Def::Enum(e) = def
                    && self.enums[e.0]
                        .variants
                        .iter()
                        .any(|v| v.name == method.name.name)
                {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "`{name}` already has a variant named `{}`",
                            method.name.name
                        ))
                        .with_code("E0238")
                        .with_primary(method.name.span, "")
                        .with_help("methods and variants of an enum need different names"),
                    );
                    continue;
                }
                let id = FnId(self.sigs.len());
                let mut sig = self.signature(method, impl_params.clone(), Some(def));
                // A trait's methods are as visible as the trait.
                sig.public |= decl.trait_name.is_some();
                self.sigs.push(sig);
                self.fn_decls.push(method);
                self.methods.insert(key, id);
            }
            if let Some(trait_name) = &decl.trait_name
                && let Some(trait_id) = self.resolve_trait(trait_name)
            {
                self.check_trait_impl(decl, trait_id, def);
            }
        }
    }

    /// Resolves a function's signature. `outer_params` are the type
    /// parameters of the surrounding `impl`, which come before the
    /// function's own; `owner` is the type a method belongs to.
    fn signature(
        &mut self,
        f: &ast::FnDecl,
        outer_params: Vec<String>,
        owner: Option<Def>,
    ) -> Signature {
        let outer_count = outer_params.len();
        let mut type_params = outer_params;
        type_params.extend(self.type_param_names(&f.type_params));
        self.type_params = type_params;
        let mut bounds = vec![Vec::new(); outer_count];
        bounds.extend(self.resolve_bounds(&f.bounds));
        self.type_bounds = bounds.clone();
        let mut params: Vec<ParamSig> = Vec::new();
        let receiver = match (owner, &f.receiver) {
            (Some(def), Some(receiver)) => {
                let ty = self.own_type(def);
                params.push(ParamSig {
                    name: "self".into(),
                    ty,
                    span: receiver.span,
                });
                Some(receiver.mutable)
            }
            _ => None,
        };
        for param in &f.params {
            if let Some(previous) = params.iter().find(|p| p.name == param.name.name) {
                self.diags.push(
                    Diagnostic::error(format!(
                        "parameter `{}` is declared more than once",
                        param.name.name
                    ))
                    .with_code("E0204")
                    .with_primary(param.name.span, "duplicate parameter")
                    .with_secondary(previous.span, "first declared here"),
                );
            }
            let ty = self.resolve_type(&param.ty, "a parameter");
            params.push(ParamSig {
                name: param.name.name.clone(),
                ty,
                span: param.span,
            });
        }
        let ret = match &f.ret {
            Some(ty) => self.resolve_type(ty, "return"),
            None => Type::Void,
        };
        let mut name = match owner {
            Some(def) => format!("{}.{}", self.def_name(def), f.name.name),
            None => f.name.name.clone(),
        };
        // Functions of other modules than the root get the module's name.
        if self.module != 0 {
            name = format!("{}.{name}", self.module_names[self.module]);
        }
        Signature {
            name,
            name_span: f.name.span,
            type_params: std::mem::take(&mut self.type_params),
            params,
            ret,
            ret_span: f.ret.as_ref().map(ast::TypeExpr::span),
            receiver,
            method: owner.is_some(),
            module: self.module,
            public: f.public,
            bounds,
        }
    }

    /// Resolves a written type. `what` describes where it appears; `void` is
    /// only accepted for return types. Type parameters of the declaration
    /// being checked (`self.type_params`) are in scope.
    pub(super) fn resolve_type(&mut self, ty: &ast::TypeExpr, what: &str) -> Type {
        if let Some(ret) = &ty.ret {
            let mut args: Vec<Type> = ty
                .args
                .iter()
                .map(|a| self.resolve_type(a, "a parameter"))
                .collect();
            args.push(self.resolve_type(ret, "return"));
            if args.contains(&Type::Error) {
                return Type::Error;
            }
            return self.types.intern(Def::Fn, args);
        }
        let name = ty.name.name.as_str();
        let param = self.type_params.iter().position(|p| p == name);
        let builtin = Type::from_name(name);
        if (param.is_some() || builtin.is_some()) && !ty.args.is_empty() {
            self.diags.push(
                Diagnostic::error(format!("`{name}` does not take type arguments"))
                    .with_code("E0225")
                    .with_primary(ty.span(), "")
                    .with_help(format!("write just `{name}`")),
            );
            return Type::Error;
        }
        if let Some(i) = param {
            return Type::Param(u32::try_from(i).expect("few type parameters"));
        }
        match builtin {
            Some(Type::Void) if what != "return" => {
                self.diags.push(
                    Diagnostic::error(format!("`void` cannot be the type of {what}"))
                        .with_code("E0213")
                        .with_primary(ty.span(), "`void` has no values")
                        .with_note("`void` is only valid as a function return type"),
                );
                Type::Error
            }
            Some(t) => t,
            None => {
                let def = match self.qualified_type(name, ty.name.span) {
                    Ok(Some(def)) => def,
                    Err(()) => return Type::Error,
                    Ok(None) => match self.type_by_name.get(name).copied() {
                        Some(def) => def,
                        None => {
                            self.unknown_type(name, ty.name.span);
                            return Type::Error;
                        }
                    },
                };
                let expected = self.def_type_params(def).len();
                let args: Vec<Type> = ty
                    .args
                    .iter()
                    .map(|a| self.resolve_type(a, "a type argument"))
                    .collect();
                if args.len() != expected {
                    let mut diag = Diagnostic::error(format!(
                        "`{name}` takes {} but {} supplied",
                        plural_type_args(expected),
                        super::supplied(args.len())
                    ))
                    .with_code("E0225")
                    .with_primary(
                        ty.span(),
                        format!("expected {}", plural_type_args(expected)),
                    );
                    if expected > 0 {
                        let params = self.def_type_params(def).join(", ");
                        diag =
                            diag.with_help(format!("write `{name}[{params}]` with actual types"));
                    }
                    self.diags.push(diag);
                    return Type::Error;
                }
                if args.contains(&Type::Error) {
                    return Type::Error;
                }
                if def == Def::Map && !matches!(args[0], Type::I64 | Type::Bool | Type::String) {
                    let key = self.show(args[0]);
                    self.diags.push(
                        Diagnostic::error(format!("`{key}` cannot be a map key"))
                            .with_code("E0236")
                            .with_primary(ty.args[0].span(), "")
                            .with_note("map keys are `i64`, `bool` or `string`"),
                    );
                    return Type::Error;
                }
                self.types.intern(def, args)
            }
        }
    }

    pub(super) fn def_type_params(&self, def: Def) -> &[String] {
        match def {
            Def::Struct(id) => &self.structs[id.0].type_params,
            Def::Enum(id) => &self.enums[id.0].type_params,
            Def::List | Def::Box => &self.list_type_params,
            // Function types are written `fn(...) -> R`, never by name.
            Def::Fn => &[],
            Def::Map => &self.map_type_params,
        }
    }

    pub(super) fn unknown_type(&mut self, name: &str, span: Span) {
        let mut diag = Diagnostic::error(format!("unknown type `{name}`"))
            .with_code("E0212")
            .with_primary(span, "not a known type");
        let declared = suggest::similar(name, self.type_by_name.keys().map(String::as_str));
        diag = match declared.or(suggest::type_name(name)) {
            Some(s) => diag.with_help(format!("did you mean `{s}`?")),
            None => diag.with_note(
                "the available types are `i64`, `bool`, `string`, `void`, `Option`, `Result`, \
                 `List`, `Map` and the declared structs and enums",
            ),
        };
        self.diags.push(diag);
    }

    pub(super) fn check_entry(&mut self) -> Option<FnId> {
        let Some(&id) = self.by_name.get("main") else {
            self.diags.push(
                Diagnostic::error("no `main` function found")
                    .with_code("E0210")
                    .with_note("every Brim program starts running at its `main` function")
                    .with_help("add an entry point: `fn main() { ... }`"),
            );
            return None;
        };
        let sig = &self.sigs[id.0];
        if let (Some(first), Some(last)) = (sig.params.first(), sig.params.last()) {
            let span = first.span.to(last.span);
            self.diags.push(
                Diagnostic::error("`main` cannot take parameters")
                    .with_code("E0211")
                    .with_primary(span, "remove these parameters")
                    .with_note("command-line arguments are not supported yet"),
            );
        }
        if !sig.type_params.is_empty() {
            self.diags.push(
                Diagnostic::error("`main` cannot be generic")
                    .with_code("E0211")
                    .with_primary(sig.name_span, "remove the type parameters"),
            );
        }
        if !matches!(sig.ret, Type::Void | Type::Error)
            && let Some(span) = sig.ret_span
        {
            self.diags.push(
                Diagnostic::error("`main` cannot return a value")
                    .with_code("E0211")
                    .with_primary(span, "remove this return type")
                    .with_note("exit codes are not supported yet"),
            );
        }
        Some(id)
    }
}

fn plural_type_args(n: usize) -> String {
    super::plural(n, "type argument")
}

fn find_cycle(
    edges: &[Vec<(String, Option<Span>, usize)>],
    current: usize,
    target: usize,
    path: &mut Vec<(usize, usize)>,
    visited: &mut [bool],
) -> bool {
    if visited[current] {
        return false;
    }
    visited[current] = true;
    for (e, (_, _, next)) in edges[current].iter().enumerate() {
        path.push((current, e));
        if *next == target || find_cycle(edges, *next, target, path, visited) {
            return true;
        }
        path.pop();
    }
    false
}
