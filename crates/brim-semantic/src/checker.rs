mod branches;
mod collections;
mod consts;
mod decls;
mod editor;
mod exprs;
mod matching;
mod strings;
mod traits;
mod values;

use std::collections::HashMap;

use brim_ast as ast;
use brim_diagnostics::{Diagnostic, Span, has_errors};

use crate::checked::{
    CheckedEnum, CheckedExpr, CheckedExprKind, CheckedField, CheckedFn, CheckedProgram,
    CheckedStmt, CheckedStruct, CheckedVariant, FnId, Local, LocalId,
};
use crate::{Def, EnumId, StructId, Type, TypeTable};

/// A resolved function signature.
struct Signature {
    /// `name`, or `Type.name` for a method.
    name: String,
    name_span: Span,
    type_params: Vec<String>,
    params: Vec<ParamSig>,
    ret: Type,
    /// Span of the `-> type` annotation, if written.
    ret_span: Option<Span>,
    /// For methods with `self`: whether it is `mut self`. `self` is then the
    /// first parameter.
    receiver: Option<bool>,
    /// Whether it is a method (declared in an `impl` block).
    method: bool,
    /// The module that declares the function.
    module: usize,
    /// Declared with `pub`.
    public: bool,
    /// The traits each type parameter must implement.
    bounds: Vec<Vec<traits::TraitId>>,
}

struct ParamSig {
    name: String,
    ty: Type,
    span: Span,
}

/// A declared struct.
struct StructInfo {
    name: String,
    name_span: Span,
    span: Span,
    type_params: Vec<String>,
    fields: Vec<FieldInfo>,
    module: usize,
    public: bool,
}

struct FieldInfo {
    name: String,
    name_span: Span,
    ty: Type,
    /// Declared with `pub`: usable outside the struct's module.
    public: bool,
    span: Span,
}

/// A declared or built-in enum.
struct EnumInfo {
    name: String,
    /// `None` for the built-in enums, which have no source location.
    name_span: Option<Span>,
    type_params: Vec<String>,
    variants: Vec<VariantInfo>,
    /// `usize::MAX` for the built-in enums.
    module: usize,
    public: bool,
}

struct VariantInfo {
    name: String,
    fields: Vec<Type>,
    span: Option<Span>,
    /// The span of the name, for the built-in enums none.
    name_span: Option<Span>,
}

pub(crate) struct Checker<'a> {
    diags: Vec<Diagnostic>,
    types: TypeTable,
    /// One entry per AST struct, in declaration order.
    structs: Vec<StructInfo>,
    /// The built-in enums, then one entry per AST enum.
    enums: Vec<EnumInfo>,
    /// Struct and enum names to their declarations.
    type_by_name: HashMap<String, Def>,
    /// Variants usable without their enum's name: `Some`, `None`, `Ok`, `Err`.
    prelude_variants: HashMap<&'static str, (EnumId, usize)>,
    /// One signature per AST function, in declaration order.
    sigs: Vec<Signature>,
    /// Function names to the first function declared with that name.
    by_name: HashMap<&'a str, FnId>,
    /// The declaration of every function, free functions first, then
    /// methods; indexed by `FnId` like `sigs`.
    fn_decls: Vec<&'a ast::FnDecl>,
    /// The functions of `fn` expressions, with ids after `fn_decls`;
    /// `None` while the body is being checked.
    lambdas: Vec<Option<CheckedFn>>,
    /// The name of the function being checked, for naming the functions
    /// of its `fn` expressions.
    current_fn_name: String,
    /// Methods by type and name.
    methods: HashMap<(Def, String), FnId>,
    /// Built-in traits first, then the declared ones.
    traits: Vec<traits::TraitInfo>,
    trait_by_name: HashMap<String, traits::TraitId>,
    /// Which types implement which traits.
    trait_impls: std::collections::HashSet<(traits::TraitId, Def)>,
    /// What an editor needs, when checking for one (`index_modules`).
    index: Option<crate::checked::EditorIndex>,
    /// Modules whose `pub fn`s are served with `serve_api`.
    api_modules: std::collections::BTreeSet<usize>,
    /// The bounds of the type parameters in scope.
    type_bounds: Vec<Vec<traits::TraitId>>,
    /// Every module's constants.
    consts: Vec<consts::ConstInfo<'a>>,
    /// Type parameter names of the declaration being checked, for resolving
    /// and showing `Type::Param`.
    type_params: Vec<String>,
    /// `["T"]`, the type parameters of `List`.
    list_type_params: Vec<String>,
    /// `["K", "V"]`, the type parameters of `Map`.
    map_type_params: Vec<String>,
    /// Set while checking a value whose expected type had an error: types
    /// that cannot be inferred are then not reported.
    quiet_inference: bool,
    /// The module being checked. While it is, `type_by_name`, `by_name` and
    /// `aliases` hold its scope; the other modules' scopes are in `scopes`.
    module: usize,
    /// Dotted module names; the root module's is empty.
    module_names: Vec<String>,
    scopes: Vec<ModuleScope<'a>>,
    /// Imported modules, by the name they are used under.
    aliases: HashMap<String, usize>,
}

/// The names visible in one module.
#[derive(Default)]
struct ModuleScope<'a> {
    types: HashMap<String, Def>,
    fns: HashMap<&'a str, FnId>,
    aliases: HashMap<String, usize>,
    /// The module's own items, for imports from other modules.
    own_types: HashMap<String, Def>,
    own_fns: HashMap<&'a str, FnId>,
    /// The module's constants, by name: indices into `Checker::consts`.
    consts: HashMap<String, usize>,
}

/// A module to check: its dotted name (empty for the root module, which
/// contains `main`) and its syntax tree.
#[derive(Debug)]
pub struct ModuleAst {
    pub name: String,
    pub program: ast::Program,
}

/// Per-function checking state.
#[derive(Default)]
struct FnCtx {
    locals: Vec<Local>,
    /// Whether each local is ever assigned after its declaration.
    reassigned: Vec<bool>,
    scopes: Vec<HashMap<String, LocalId>>,
    /// One entry per enclosing loop: whether it contains a `break`.
    loops: Vec<bool>,
    /// The first `param_count` locals are the parameters.
    param_count: usize,
    ret: Type,
    ret_span: Option<Span>,
    /// In a `fn` expression: the function around it, whose variables it
    /// can use (capture).
    parent: Option<Box<FnCtx>>,
    /// Captured variables: the parent's local and the copy here.
    captures: Vec<(LocalId, LocalId)>,
}

impl FnCtx {
    /// A variable of this function.
    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    /// Whether `name` is a variable here or in a function around this one.
    fn is_variable(&self, name: &str) -> bool {
        self.lookup(name).is_some() || self.parent.as_ref().is_some_and(|p| p.is_variable(name))
    }

    /// A variable for reading: one of this function's, or a copy of one
    /// from the functions around it, captured on first use.
    fn lookup_capturing(&mut self, name: &str) -> Option<LocalId> {
        if let Some(id) = self.lookup(name) {
            return Some(id);
        }
        let parent = self.parent.as_mut()?;
        let outer = parent.lookup_capturing(name)?;
        let captured = Local {
            mutable: false,
            ..parent.locals[outer.0].clone()
        };
        let id = LocalId(self.locals.len());
        self.locals.push(captured);
        self.reassigned.push(false);
        // Scope 0 holds the captures, outside the parameters' scope.
        self.scopes[0].insert(name.to_string(), id);
        self.captures.push((outer, id));
        Some(id)
    }

    fn visible_names(&self) -> impl Iterator<Item = &str> {
        self.scopes
            .iter()
            .flat_map(|scope| scope.keys().map(String::as_str))
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// `"1 was"` or `"3 were"`.
fn supplied(n: usize) -> String {
    if n == 1 {
        "1 was".to_string()
    } else {
        format!("{n} were")
    }
}

fn error_expr(span: Span) -> CheckedExpr {
    CheckedExpr {
        kind: CheckedExprKind::Bool(false),
        ty: Type::Error,
        span,
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn check(
        modules: &'a [ModuleAst],
        tests: bool,
    ) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
        let (program, diags, _) = Self::check_program(modules, tests, false);
        (program, diags)
    }

    /// Checks the program; with `editor`, also gathers an [`EditorIndex`].
    pub(crate) fn check_program(
        modules: &'a [ModuleAst],
        tests: bool,
        editor: bool,
    ) -> (
        Option<CheckedProgram>,
        Vec<Diagnostic>,
        Option<crate::checked::EditorIndex>,
    ) {
        let mut checker = Checker {
            diags: Vec::new(),
            types: TypeTable::default(),
            structs: Vec::new(),
            enums: Vec::new(),
            type_by_name: HashMap::new(),
            prelude_variants: HashMap::new(),
            sigs: Vec::new(),
            by_name: HashMap::new(),
            fn_decls: Vec::new(),
            lambdas: Vec::new(),
            current_fn_name: String::new(),
            methods: HashMap::new(),
            traits: traits::builtin_traits(),
            trait_by_name: traits::builtin_trait_names(),
            trait_impls: std::collections::HashSet::new(),
            api_modules: std::collections::BTreeSet::new(),
            index: editor.then(Default::default),
            type_bounds: Vec::new(),
            consts: Vec::new(),
            type_params: Vec::new(),
            list_type_params: vec!["T".into()],
            map_type_params: vec!["K".into(), "V".into()],
            quiet_inference: false,
            module: 0,
            module_names: modules.iter().map(|m| m.name.clone()).collect(),
            scopes: Vec::new(),
            aliases: HashMap::new(),
        };
        checker.add_prelude();
        for _ in modules {
            let mut scope = ModuleScope::default();
            for (name, def) in [
                ("Option", Def::Enum(EnumId(0))),
                ("Result", Def::Enum(EnumId(1))),
                ("List", Def::List),
                ("Map", Def::Map),
                ("Set", Def::Set),
                ("Box", Def::Box),
            ] {
                scope.types.insert(name.into(), def);
            }
            checker.scopes.push(scope);
        }

        // Declarations happen in passes over all modules, so that every
        // module can use what the others declare.
        let mut bases = Vec::with_capacity(modules.len());
        for (m, module) in modules.iter().enumerate() {
            checker.enter(m);
            bases.push(checker.declare_types(&module.program));
            checker.declare_functions(&module.program);
            checker.leave();
            let scope = &mut checker.scopes[m];
            scope.own_types = scope
                .types
                .iter()
                .filter(|(_, d)| matches!(d, Def::Struct(_) | Def::Enum(_)))
                .filter(|(n, _)| !matches!(n.as_str(), "Option" | "Result"))
                .map(|(n, d)| (n.clone(), *d))
                .collect();
            scope.own_fns = scope.fns.clone();
        }
        // The prelude's public types, such as `StringBuilder`, are known in
        // every module that does not declare its own.
        if let Some(prelude) = checker.module_names.iter().position(|n| n == "std.prelude") {
            let types: Vec<(String, Def)> = checker.scopes[prelude]
                .own_types
                .iter()
                .filter(|(_, d)| checker.def_public(**d))
                .map(|(n, d)| (n.clone(), *d))
                .collect();
            for (m, scope) in checker.scopes.iter_mut().enumerate() {
                if m != prelude {
                    for (name, def) in &types {
                        scope.types.entry(name.clone()).or_insert(*def);
                    }
                }
            }
        }
        for (m, module) in modules.iter().enumerate() {
            checker.collect_consts(m, &module.program);
        }
        checker.collect_reexports(modules);
        for (m, module) in modules.iter().enumerate() {
            checker.resolve_imports(m, &module.program);
        }
        for (m, module) in modules.iter().enumerate() {
            checker.enter(m);
            let (struct_base, enum_base) = bases[m];
            checker.define_types(&module.program, struct_base, enum_base);
            checker.leave();
        }
        checker.check_recursive_types();
        for (m, module) in modules.iter().enumerate() {
            checker.enter(m);
            checker.collect_traits(&module.program);
            checker.leave();
        }
        for (m, module) in modules.iter().enumerate() {
            checker.enter(m);
            checker.collect_functions(&module.program);
            checker.leave();
        }
        for (m, module) in modules.iter().enumerate() {
            checker.enter(m);
            checker.collect_impls(&module.program);
            checker.leave();
        }

        checker.enter(0);
        let entry = if tests && !checker.by_name.contains_key("main") {
            None
        } else {
            checker.check_entry()
        };
        checker.leave();
        let tests = if tests {
            checker.collect_tests()
        } else {
            Vec::new()
        };
        let decls = checker.fn_decls.clone();
        let mut functions: Vec<CheckedFn> = Vec::with_capacity(decls.len());
        for (i, f) in decls.iter().enumerate() {
            checker.enter(checker.sigs[i].module);
            functions.push(checker.check_fn(f, FnId(i)));
            checker.leave();
        }
        // Constants nothing uses are checked too.
        for id in 0..checker.consts.len() {
            checker.enter(checker.consts[id].module);
            checker.const_value(id, None);
            checker.leave();
        }
        functions.extend(
            std::mem::take(&mut checker.lambdas)
                .into_iter()
                .map(|f| f.expect("every `fn` expression is checked")),
        );

        let api = checker.api_functions();
        let index = checker.finish_index();
        let mut diags = checker.diags;
        // Source order; diagnostics without a location go last.
        diags.sort_by_key(|d| d.span().map_or(usize::MAX, |s| s.start));
        if has_errors(&diags) {
            return (None, diags, index);
        }
        let structs = checker
            .structs
            .into_iter()
            .map(|s| CheckedStruct {
                name: s.name,
                span: s.span,
                module: s.module,
                public: s.public,
                type_params: s.type_params,
                fields: s
                    .fields
                    .into_iter()
                    .map(|f| CheckedField {
                        name: f.name,
                        ty: f.ty,
                    })
                    .collect(),
            })
            .collect();
        let enums = checker
            .enums
            .into_iter()
            .map(|e| CheckedEnum {
                name: e.name,
                module: (e.module != usize::MAX).then_some(e.module),
                public: e.public,
                type_params: e.type_params,
                variants: e
                    .variants
                    .into_iter()
                    .map(|v| CheckedVariant {
                        name: v.name,
                        fields: v.fields,
                    })
                    .collect(),
            })
            .collect();
        let module_fns = checker
            .scopes
            .iter()
            .map(|s| {
                s.own_fns
                    .iter()
                    .map(|(name, id)| ((*name).to_string(), *id))
                    .collect()
            })
            .collect();
        (
            Some(CheckedProgram {
                module_fns,
                api,
                types: checker.types,
                structs,
                enums,
                functions,
                entry,
                modules: checker.module_names,
                tests,
                methods: checker.methods,
            }),
            diags,
            index,
        )
    }

    /// The functions `serve_api` serves: the non-generic `pub fn`s, other
    /// than methods, of the modules it was called with.
    pub(super) fn api_candidates(&self, module: usize) -> Vec<FnId> {
        (0..self.sigs.len())
            .filter(|&i| {
                let sig = &self.sigs[i];
                sig.module == module && sig.public && !sig.method
            })
            .map(FnId)
            .collect()
    }

    fn api_functions(&self) -> Vec<crate::checked::ApiFn> {
        let mut api = Vec::new();
        for &module in &self.api_modules {
            for id in self.api_candidates(module) {
                let sig = &self.sigs[id.0];
                api.push(crate::checked::ApiFn {
                    module,
                    function: id,
                    // Without the module's name in front.
                    name: sig.name.rsplit('.').next().unwrap_or(&sig.name).to_string(),
                    params: sig.params.iter().map(|p| (p.name.clone(), p.ty)).collect(),
                    ret: sig.ret,
                });
            }
        }
        api
    }

    /// The functions `brim test` runs: free functions named `test_...`.
    /// They must take no parameters and return nothing.
    fn collect_tests(&mut self) -> Vec<FnId> {
        let mut tests = Vec::new();
        for (i, sig) in self.sigs.iter().enumerate() {
            let short = sig.name.rsplit('.').next().unwrap_or(&sig.name);
            if sig.method || !short.starts_with("test_") {
                continue;
            }
            if !sig.params.is_empty()
                || !sig.type_params.is_empty()
                || !matches!(sig.ret, Type::Void | Type::Error)
            {
                self.diags.push(
                    Diagnostic::error(format!(
                        "test `{}` must take no parameters and return nothing",
                        sig.name
                    ))
                    .with_code("E0244")
                    .with_primary(sig.name_span, "")
                    .with_note("functions whose names start with `test_` are tests"),
                );
                continue;
            }
            tests.push(FnId(i));
        }
        tests
    }

    /// Whether the current module is part of the standard library
    /// (`std.net`, ...).
    fn in_std_module(&self) -> bool {
        self.module_names[self.module].starts_with("std.")
    }

    /// Makes module `m`'s scope the current one.
    fn enter(&mut self, m: usize) {
        self.module = m;
        let scope = &mut self.scopes[m];
        std::mem::swap(&mut self.type_by_name, &mut scope.types);
        std::mem::swap(&mut self.by_name, &mut scope.fns);
        std::mem::swap(&mut self.aliases, &mut scope.aliases);
    }

    /// Puts the current module's scope back.
    fn leave(&mut self) {
        let scope = &mut self.scopes[self.module];
        std::mem::swap(&mut self.type_by_name, &mut scope.types);
        std::mem::swap(&mut self.by_name, &mut scope.fns);
        std::mem::swap(&mut self.aliases, &mut scope.aliases);
    }

    /// The module an import names, if it exists and is not `m` itself.
    fn import_target(&self, m: usize, import: &ast::ImportDecl) -> Option<usize> {
        let joined = import
            .path
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(".");
        self.module_names
            .iter()
            .position(|n| *n == joined && !n.is_empty())
            .filter(|t| *t != m)
    }

    /// `pub import a.{X}` makes `X` an item of the importing module too.
    /// Repeated until nothing changes, so re-exports of re-exports work in
    /// any module order. Errors are reported with the other imports.
    fn collect_reexports(&mut self, modules: &'a [ModuleAst]) {
        let mut changed = true;
        while changed {
            changed = false;
            for (m, module) in modules.iter().enumerate() {
                for import in module.program.imports.iter().filter(|i| i.public) {
                    let Some(target) = self.import_target(m, import) else {
                        continue;
                    };
                    for item in import.items.iter().flatten() {
                        let name = item.name.as_str();
                        if let Some(&def) = self.scopes[target].own_types.get(name)
                            && self.def_public(def)
                            && !self.scopes[m].own_types.contains_key(name)
                        {
                            self.scopes[m].own_types.insert(name.to_string(), def);
                            changed = true;
                        }
                        if let Some(&f) = self.scopes[target].own_fns.get(name)
                            && self.fn_decls[f.0].public
                            && !self.scopes[m].own_fns.contains_key(name)
                        {
                            self.scopes[m].own_fns.insert(name, f);
                            changed = true;
                        }
                        if let Some(&c) = self.scopes[target].consts.get(name)
                            && self.consts[c].decl.public
                            && !self.scopes[m].consts.contains_key(name)
                        {
                            self.scopes[m].consts.insert(name.to_string(), c);
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    /// Adds module `m`'s imports to its scope.
    fn resolve_imports(&mut self, m: usize, program: &'a ast::Program) {
        for import in &program.imports {
            let path: Vec<&str> = import.path.iter().map(|p| p.name.as_str()).collect();
            let joined = path.join(".");
            let Some(target) = self
                .module_names
                .iter()
                .position(|n| *n == joined && !n.is_empty())
            else {
                // The driver reports files it cannot find; this covers the rest.
                self.diags.push(
                    Diagnostic::error(format!("cannot find module `{joined}`"))
                        .with_code("E0242")
                        .with_primary(import.span, ""),
                );
                continue;
            };
            if target == m {
                self.diags.push(
                    Diagnostic::error("a module cannot import itself")
                        .with_code("E0242")
                        .with_primary(import.span, ""),
                );
                continue;
            }
            match &import.items {
                None => {
                    let alias = import
                        .alias
                        .as_ref()
                        .unwrap_or_else(|| import.path.last().expect("a path"));
                    if let Some(&previous) = self.scopes[m].aliases.get(&alias.name)
                        && previous != target
                    {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "`{}` is imported more than once",
                                alias.name
                            ))
                            .with_code("E0243")
                            .with_primary(alias.span, "")
                            .with_help("give one of the modules another name with `as`"),
                        );
                        continue;
                    }
                    self.scopes[m].aliases.insert(alias.name.clone(), target);
                }
                Some(items) => {
                    for item in items {
                        let def = self.scopes[target].own_types.get(&item.name).copied();
                        let func = self.scopes[target].own_fns.get(item.name.as_str()).copied();
                        let konst = self.scopes[target].consts.get(&item.name).copied();
                        let public = match (def, func, konst) {
                            (Some(def), _, _) => self.def_public(def),
                            (None, Some(f), _) => self.fn_decls[f.0].public,
                            (None, None, Some(c)) => self.consts[c].decl.public,
                            (None, None, None) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "module `{joined}` has no item `{}`",
                                        item.name
                                    ))
                                    .with_code("E0242")
                                    .with_primary(item.span, ""),
                                );
                                continue;
                            }
                        };
                        if !public {
                            self.private_item(&item.name, &joined, item.span);
                            continue;
                        }
                        let scope = &mut self.scopes[m];
                        let taken = scope.types.contains_key(&item.name)
                            || scope.fns.contains_key(item.name.as_str())
                            || (konst.is_some()
                                && scope
                                    .consts
                                    .get(&item.name)
                                    .is_some_and(|c| Some(*c) != konst));
                        if taken {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "`{}` is already defined in this module",
                                    item.name
                                ))
                                .with_code("E0243")
                                .with_primary(item.span, "imported here")
                                .with_help(format!(
                                    "import the module instead and write `{}.{}`",
                                    path.last().expect("a path"),
                                    item.name
                                )),
                            );
                            continue;
                        }
                        if let Some(def) = def {
                            scope.types.insert(item.name.clone(), def);
                        }
                        if let Some(func) = func {
                            let name = self.fn_decls[func.0].name.name.as_str();
                            self.scopes[m].fns.insert(name, func);
                        }
                        if let Some(c) = konst {
                            self.scopes[m].consts.insert(item.name.clone(), c);
                        }
                    }
                }
            }
        }
    }

    fn def_public(&self, def: Def) -> bool {
        match def {
            Def::Struct(id) => self.structs[id.0].public,
            Def::Enum(id) => self.enums[id.0].public,
            Def::List | Def::Map | Def::Set | Def::Box | Def::Fn | Def::Tuple => true,
        }
    }

    fn def_module(&self, def: Def) -> usize {
        match def {
            Def::Struct(id) => self.structs[id.0].module,
            Def::Enum(id) => self.enums[id.0].module,
            Def::List | Def::Map | Def::Set | Def::Box | Def::Fn | Def::Tuple => usize::MAX,
        }
    }

    fn private_item(&mut self, name: &str, module: &str, span: Span) {
        self.diags.push(
            Diagnostic::error(format!("`{name}` is private to module `{module}`"))
                .with_code("E0241")
                .with_primary(span, "not visible here")
                .with_help(format!(
                    "declare it with `pub` in `{module}` to use it elsewhere"
                )),
        );
    }

    /// Resolves a module-qualified type name, `alias.Type`, reporting
    /// errors. `None` if `name` is not qualified or after an error (with
    /// `reported` set).
    fn qualified_type(&mut self, name: &str, span: Span) -> Result<Option<Def>, ()> {
        let Some((alias, item)) = name.split_once('.') else {
            return Ok(None);
        };
        let Some(&target) = self.aliases.get(alias) else {
            self.diags.push(
                Diagnostic::error(format!("unknown module `{alias}`"))
                    .with_code("E0242")
                    .with_primary(span, "")
                    .with_help(format!("import it first: `import {alias}`")),
            );
            return Err(());
        };
        let module = self.module_names[target].clone();
        let Some(def) = self.scopes[target].own_types.get(item).copied() else {
            self.diags.push(
                Diagnostic::error(format!("module `{module}` has no type `{item}`"))
                    .with_code("E0212")
                    .with_primary(span, "not a known type"),
            );
            return Err(());
        };
        if !self.def_public(def) {
            self.private_item(item, &module, span);
            return Err(());
        }
        Ok(Some(def))
    }

    /// A type as users write it, for messages.
    fn show(&self, ty: Type) -> String {
        match ty {
            Type::Param(i) => self
                .type_params
                .get(i as usize)
                .cloned()
                .unwrap_or_else(|| format!("T{i}")),
            Type::Named(id) => {
                let named = self.types.get(id);
                if named.def == Def::Tuple {
                    let items: Vec<String> = named.args.iter().map(|t| self.show(*t)).collect();
                    return format!("({})", items.join(", "));
                }
                if named.def == Def::Fn {
                    let (ret, params) = named.args.split_last().expect("a return type");
                    let params: Vec<String> = params.iter().map(|t| self.show(*t)).collect();
                    return match ret {
                        Type::Void => format!("fn({})", params.join(", ")),
                        ret => format!("fn({}) -> {}", params.join(", "), self.show(*ret)),
                    };
                }
                let name = self.def_name(named.def);
                if named.args.is_empty() {
                    name.to_string()
                } else {
                    let args: Vec<String> = named.args.iter().map(|t| self.show(*t)).collect();
                    format!("{name}[{}]", args.join(", "))
                }
            }
            other => other.to_string(),
        }
    }

    fn def_name(&self, def: Def) -> &str {
        match def {
            Def::Struct(id) => &self.structs[id.0].name,
            Def::Enum(id) => &self.enums[id.0].name,
            Def::List => "List",
            Def::Map => "Map",
            Def::Set => "Set",
            Def::Box => "Box",
            Def::Fn => "fn",
            Def::Tuple => "tuple",
        }
    }

    /// The struct a type refers to, with its type arguments.
    fn as_struct(&self, ty: Type) -> Option<(StructId, Vec<Type>)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        match named.def {
            Def::Struct(s) => Some((s, named.args.clone())),
            Def::Enum(_) | Def::List | Def::Map | Def::Set | Def::Box | Def::Fn | Def::Tuple => {
                None
            }
        }
    }

    /// The enum a type refers to, with its type arguments.
    fn as_enum(&self, ty: Type) -> Option<(EnumId, Vec<Type>)> {
        let Type::Named(id) = ty else { return None };
        let named = self.types.get(id);
        match named.def {
            Def::Enum(e) => Some((e, named.args.clone())),
            Def::Struct(_) | Def::List | Def::Map | Def::Set | Def::Box | Def::Fn | Def::Tuple => {
                None
            }
        }
    }

    // ---- function bodies ----------------------------------------------

    fn check_fn(&mut self, f: &ast::FnDecl, id: FnId) -> CheckedFn {
        let sig = &self.sigs[id.0];
        self.type_params = sig.type_params.clone();
        self.type_bounds = sig.bounds.clone();
        self.current_fn_name = sig.name.clone();
        let mut ctx = FnCtx {
            scopes: vec![HashMap::new()],
            param_count: sig.params.len(),
            ret: sig.ret,
            ret_span: sig.ret_span,
            ..FnCtx::default()
        };
        let mut_self = sig.receiver == Some(true);
        for (index, param) in sig.params.iter().enumerate() {
            let local = LocalId(ctx.locals.len());
            ctx.locals.push(Local {
                name: param.name.clone(),
                ty: param.ty,
                // `mut self` lets the method change the caller's value.
                mutable: mut_self && index == 0,
                span: param.span,
            });
            ctx.reassigned.push(false);
            // For duplicate parameters (already reported), the first one wins.
            ctx.scopes[0].entry(param.name.clone()).or_insert(local);
        }
        let param_count = ctx.locals.len();

        if let Some(extern_span) = f.external {
            self.check_extern(f, extern_span, &ctx);
            let type_params = std::mem::take(&mut self.type_params);
            return CheckedFn {
                name: self.sigs[id.0].name.clone(),
                mut_self,
                span: f.span,
                type_params,
                param_count,
                ret: ctx.ret,
                locals: ctx.locals,
                body: Vec::new(),
                captures: None,
                external: Some(f.name.name.clone()),
            };
        }

        // The body shares its scope with the parameters, so `let a` in the
        // body conflicts with a parameter `a`.
        let (body, diverges) = self.check_stmts(&f.body.stmts, &mut ctx);

        let skip = usize::from(mut_self);
        for (local, reassigned) in ctx.locals.iter().zip(&ctx.reassigned).skip(skip) {
            if local.mutable && !reassigned {
                self.diags.push(
                    Diagnostic::warning(format!(
                        "variable `{}` is declared `mut` but never reassigned",
                        local.name
                    ))
                    .with_code("W0003")
                    .with_primary(local.span, "")
                    .with_help("remove `mut`"),
                );
            }
        }

        if !diverges && !matches!(ctx.ret, Type::Void | Type::Error) {
            let close = Span::new(f.body.span.end.saturating_sub(1), f.body.span.end);
            let mut diag = Diagnostic::error(format!(
                "function `{}` may end without returning a value",
                f.name.name
            ))
            .with_code("E0207")
            .with_primary(close, "missing `return` before the end of the function");
            if let Some(span) = ctx.ret_span {
                let ret = self.show(ctx.ret);
                diag = diag.with_secondary(span, format!("`{ret}` must be returned"));
            }
            self.diags.push(diag);
        }

        let type_params = std::mem::take(&mut self.type_params);
        CheckedFn {
            name: self.sigs[id.0].name.clone(),
            mut_self,
            span: f.span,
            type_params,
            param_count,
            ret: ctx.ret,
            locals: ctx.locals,
            body,
            captures: None,
            external: None,
        }
    }

    /// An `extern fn` takes and returns only what C has a type for.
    fn check_extern(&mut self, f: &ast::FnDecl, extern_span: Span, ctx: &FnCtx) {
        let c_type = |ty: Type| match ty {
            Type::I64 => Some("int64_t"),
            Type::I32 => Some("int32_t"),
            Type::U8 => Some("uint8_t"),
            Type::U64 => Some("uint64_t"),
            Type::F64 => Some("double"),
            Type::Bool => Some("bool"),
            Type::String => Some("const char *"),
            Type::Error => Some(""),
            _ => None,
        };
        let not_c = |this: &Self, what: &str, ty: Type, span: Span| {
            Diagnostic::error(format!(
                "{what} of an `extern fn` cannot be `{}`",
                this.show(ty)
            ))
            .with_code("E0260")
            .with_primary(span, "")
            .with_note(
                "C functions take and return `i64`, `i32`, `u8`, `u64`, `f64`, `bool` and                  `string` (a `const char *`), and may return nothing",
            )
        };
        if let Some(param) = f.type_params.first() {
            self.diags.push(
                Diagnostic::error("an `extern fn` cannot be generic")
                    .with_code("E0260")
                    .with_primary(param.span, ""),
            );
        }
        if let Some(receiver) = &f.receiver {
            self.diags.push(
                Diagnostic::error("an `extern fn` cannot take `self`")
                    .with_code("E0260")
                    .with_primary(receiver.span, ""),
            );
        }
        if f.name.name == "main" {
            self.diags.push(
                Diagnostic::error("`main` cannot be an `extern fn`")
                    .with_code("E0260")
                    .with_primary(extern_span, ""),
            );
        }
        for local in &ctx.locals {
            if c_type(local.ty).is_none() {
                let diag = not_c(
                    self,
                    &format!("parameter `{}`", local.name),
                    local.ty,
                    local.span,
                );
                self.diags.push(diag);
            }
        }
        if ctx.ret != Type::Void && c_type(ctx.ret).is_none() {
            let span = ctx.ret_span.unwrap_or(f.name.span);
            let diag = not_c(self, "the result", ctx.ret, span);
            self.diags.push(diag);
        }
    }

    /// Checks a statement list. Returns the checked statements and whether
    /// the list *diverges*: control never continues past its end, because
    /// every path returns, breaks, continues or loops forever.
    fn check_stmts(&mut self, stmts: &[ast::Stmt], ctx: &mut FnCtx) -> (Vec<CheckedStmt>, bool) {
        let mut out = Vec::with_capacity(stmts.len());
        let mut returned_at: Option<Span> = None;
        let mut warned = false;
        for stmt in stmts {
            if let Some(span) = returned_at
                && !warned
            {
                warned = true;
                self.diags.push(
                    Diagnostic::warning("unreachable code")
                        .with_code("W0001")
                        .with_primary(stmt.span, "this statement will never run")
                        .with_secondary(span, "control never continues past this"),
                );
            }
            let (checked, returns) = self.check_stmt(stmt, ctx);
            out.push(checked);
            if returns && returned_at.is_none() {
                returned_at = Some(stmt.span);
            }
        }
        (out, returned_at.is_some())
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt, ctx: &mut FnCtx) -> (CheckedStmt, bool) {
        match &stmt.kind {
            ast::StmtKind::LetPattern {
                mutable,
                pattern,
                init,
            } => (self.check_let_pattern(*mutable, pattern, init, ctx), false),
            ast::StmtKind::Let {
                mutable,
                name,
                ty,
                init,
            } => {
                let Some(ty) = ty else {
                    // `let x = value`: the variable has the value's type.
                    let init = self.check_expr(init, ctx);
                    let inferred = if init.ty == Type::Void {
                        self.diags.push(
                            Diagnostic::error(format!("`{}` would have no value", name.name))
                                .with_code("E0213")
                                .with_primary(init.span, "this returns nothing")
                                .with_note("the function called here returns `void`"),
                        );
                        Type::Error
                    } else {
                        init.ty
                    };
                    let local = self.declare(name, inferred, *mutable, ctx);
                    return (CheckedStmt::Let { local, init }, false);
                };
                let declared = self.resolve_type(ty, "a variable");
                let expected = (declared != Type::Error).then_some(declared);
                // With a broken annotation, "cannot infer" errors are noise.
                let quiet = std::mem::replace(&mut self.quiet_inference, declared == Type::Error);
                let init = self.check_expr_with(init, expected, ctx);
                self.quiet_inference = quiet;
                self.expect_type(
                    &init,
                    declared,
                    Some(ty.span()),
                    "expected due to this type",
                );
                let local = self.declare(name, declared, *mutable, ctx);
                (CheckedStmt::Let { local, init }, false)
            }
            ast::StmtKind::Assign {
                target,
                path,
                op,
                op_span,
                value,
            } => (
                self.check_assign(target, path, *op, *op_span, value, ctx),
                false,
            ),
            ast::StmtKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                let cond = self.check_condition(cond, "if", ctx);
                let (then_body, then_diverges) = self.check_block(&then_block.stmts, ctx);
                let (else_body, else_diverges) = match else_branch.as_deref() {
                    None => (Vec::new(), false),
                    Some(ast::Stmt {
                        kind: ast::StmtKind::Block(block),
                        ..
                    }) => self.check_block(&block.stmts, ctx),
                    Some(other) => {
                        let (stmt, diverges) = self.check_stmt(other, ctx);
                        (vec![stmt], diverges)
                    }
                };
                let stmt = CheckedStmt::If {
                    cond,
                    then_body,
                    else_body,
                };
                (stmt, then_diverges && else_diverges)
            }
            ast::StmtKind::While { cond, body } => {
                let cond = self.check_condition(cond, "while", ctx);
                ctx.loops.push(false);
                let (body, _) = self.check_block(&body.stmts, ctx);
                let has_break = ctx.loops.pop().expect("pushed above");
                // `while true` without `break` never finishes.
                let forever = matches!(cond.kind, CheckedExprKind::Bool(true)) && !has_break;
                (CheckedStmt::While { cond, body }, forever)
            }
            ast::StmtKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, ctx),
            ast::StmtKind::For { var, iter, body } => self.check_for(var, iter, body, ctx),
            ast::StmtKind::Break | ast::StmtKind::Continue => {
                let is_break = matches!(stmt.kind, ast::StmtKind::Break);
                let keyword = if is_break { "break" } else { "continue" };
                match ctx.loops.last_mut() {
                    Some(has_break) => *has_break |= is_break,
                    None => self.diags.push(
                        Diagnostic::error(format!("`{keyword}` outside of a loop"))
                            .with_code("E0217")
                            .with_primary(stmt.span, format!("cannot `{keyword}` here"))
                            .with_note(format!(
                                "`{keyword}` can only be used inside a `while` loop"
                            )),
                    ),
                }
                let checked = if is_break {
                    CheckedStmt::Break
                } else {
                    CheckedStmt::Continue
                };
                (checked, true)
            }
            ast::StmtKind::Return(value) => {
                let expected = (!matches!(ctx.ret, Type::Void | Type::Error)).then_some(ctx.ret);
                let value = value
                    .as_ref()
                    .map(|v| self.check_expr_with(v, expected, ctx));
                self.check_return(value.as_ref(), stmt.span, ctx);
                (
                    CheckedStmt::Return {
                        value,
                        span: stmt.span,
                    },
                    true,
                )
            }
            ast::StmtKind::Expr(expr) => {
                let checked = self.check_expr(expr, ctx);
                let is_call = |e: &CheckedExpr| {
                    matches!(
                        e.kind,
                        CheckedExprKind::Call { .. }
                            | CheckedExprKind::CallValue { .. }
                            | CheckedExprKind::Builtin { .. }
                            | CheckedExprKind::ListOp { .. }
                            | CheckedExprKind::MapOp { .. }
                            | CheckedExprKind::CallMut { .. }
                            | CheckedExprKind::Intrinsic { .. }
                    )
                };
                // `do_something()?` is a call too: only its error is used.
                let is_call = is_call(&checked)
                    || matches!(&checked.kind, CheckedExprKind::Try { operand, .. } if is_call(operand));
                if !is_call && checked.ty != Type::Error {
                    let ty = self.show(checked.ty);
                    self.diags.push(
                        Diagnostic::warning("unused value")
                            .with_code("W0002")
                            .with_primary(checked.span, format!("this `{ty}` value is never used"))
                            .with_help("store it with `let`, or remove the expression"),
                    );
                }
                // `exit(...)` ends the program.
                let diverges = matches!(
                    checked.kind,
                    CheckedExprKind::Builtin { runtime, .. } if runtime.diverges()
                );
                (CheckedStmt::Expr(checked), diverges)
            }
            ast::StmtKind::Block(block) => {
                let (stmts, diverges) = self.check_block(&block.stmts, ctx);
                (CheckedStmt::Block(stmts), diverges)
            }
        }
    }

    /// Checks statements in a new scope.
    fn check_block(&mut self, stmts: &[ast::Stmt], ctx: &mut FnCtx) -> (Vec<CheckedStmt>, bool) {
        ctx.scopes.push(HashMap::new());
        let result = self.check_stmts(stmts, ctx);
        ctx.scopes.pop();
        result
    }

    /// Checks the condition of an `if` or `while`; it must be a `bool`.
    fn check_condition(&mut self, cond: &ast::Expr, keyword: &str, ctx: &mut FnCtx) -> CheckedExpr {
        let cond = self.check_expr(cond, ctx);
        if !matches!(cond.ty, Type::Bool | Type::Error) {
            let found = self.show(cond.ty);
            let mut diag = Diagnostic::error("mismatched types")
                .with_code("E0206")
                .with_primary(cond.span, format!("expected `bool`, found `{found}`"))
                .with_note(format!("the condition of `{keyword}` must be a `bool`"));
            if cond.ty == Type::I64 {
                diag = diag.with_help("compare explicitly, for example `x != 0`");
            }
            self.diags.push(diag);
        }
        cond
    }

    fn check_assign(
        &mut self,
        target: &ast::Ident,
        path: &[ast::PlaceElem],
        op: Option<ast::BinaryOp>,
        op_span: Span,
        value: &ast::Expr,
        ctx: &mut FnCtx,
    ) -> CheckedStmt {
        let steps: Vec<collections::StepRef<'_>> = path
            .iter()
            .map(|elem| match elem {
                ast::PlaceElem::Field(field) => collections::StepRef::Field(field),
                ast::PlaceElem::Index(index) => collections::StepRef::Index(index),
            })
            .collect();
        let Some(place) = self.resolve_place(target, &steps, ctx) else {
            let value = self.check_expr(value, ctx);
            return CheckedStmt::Expr(value);
        };
        let local = &ctx.locals[place.local.0];
        let (var_ty, mutable, decl_span) = (local.ty, local.mutable, local.span);
        let is_param = place.local.0 < ctx.param_count;
        let ty = place.ty;
        let expected = (op.is_none() && ty != Type::Error).then_some(ty);
        let value = self.check_expr_with(value, expected, ctx);

        if !mutable {
            let ty = self.show(var_ty);
            let message = if path.is_empty() {
                format!("cannot assign to immutable variable `{}`", target.name)
            } else {
                format!(
                    "cannot assign to a part of immutable variable `{}`",
                    target.name
                )
            };
            let mut diag = Diagnostic::error(message)
                .with_code("E0218")
                .with_primary(
                    target.span,
                    if path.is_empty() {
                        "cannot be reassigned"
                    } else {
                        "it cannot be changed"
                    },
                )
                .with_secondary(decl_span, "declared here");
            let captured = ctx.captures.iter().any(|(_, own)| *own == place.local);
            diag = if target.name == "self" {
                diag.with_help("declare the method with `mut self` to change `self`")
            } else if captured {
                diag.with_note("a `fn` expression works on copies of the variables it uses")
                    .with_help("return the new value from the `fn` instead, and assign it outside")
            } else if is_param {
                diag.with_note("function parameters cannot be reassigned")
                    .with_help(format!(
                        "copy it into a mutable variable: `let mut {}: {ty} = {}`",
                        target.name, target.name
                    ))
            } else {
                diag.with_help(format!("make it mutable: `let mut {}`", target.name))
            };
            self.diags.push(diag);
        } else {
            ctx.reassigned[place.local.0] = true;
        }

        let reason = if path.is_empty() {
            "variable declared here"
        } else {
            "the variable's type is declared here"
        };
        let (op, value) = match op {
            None => {
                self.expect_type(&value, ty, Some(decl_span), reason);
                (None, value)
            }
            Some(op) => {
                // Type-check `place op value` with a stand-in for the place.
                let current = CheckedExpr {
                    kind: CheckedExprKind::Bool(false),
                    ty,
                    span: target.span,
                };
                let span = target.span.to(value.span);
                let value_span = value.span;
                let combined = self.resolve_binary(op, op_span, current, value, span);
                self.expect_type(&combined, ty, Some(decl_span), reason);
                match combined.kind {
                    CheckedExprKind::Binary { op, rhs, .. } => (Some(op), *rhs),
                    // An error was reported.
                    _ => (None, error_expr(value_span)),
                }
            }
        };
        CheckedStmt::Assign { place, op, value }
    }

    fn declare(&mut self, name: &ast::Ident, ty: Type, mutable: bool, ctx: &mut FnCtx) -> LocalId {
        let id = LocalId(ctx.locals.len());
        if let Some(previous) = ctx.scopes.last().and_then(|s| s.get(&name.name)) {
            let previous_span = ctx.locals[previous.0].span;
            self.diags.push(
                Diagnostic::error(format!(
                    "variable `{}` is already declared in this scope",
                    name.name
                ))
                .with_code("E0203")
                .with_primary(name.span, "declared again here")
                .with_secondary(previous_span, "first declared here")
                .with_help("use a different name, or declare it inside a nested block"),
            );
        }
        self.note_type(name.span, ty);
        ctx.locals.push(Local {
            name: name.name.clone(),
            ty,
            mutable,
            span: name.span,
        });
        ctx.reassigned.push(false);
        ctx.scopes
            .last_mut()
            .expect("there is always a scope")
            .insert(name.name.clone(), id);
        id
    }

    fn check_return(&mut self, value: Option<&CheckedExpr>, span: Span, ctx: &FnCtx) {
        match (value, ctx.ret) {
            (_, Type::Error) | (None, Type::Void) => {}
            (None, ret) => {
                let ret = self.show(ret);
                let mut diag = Diagnostic::error("missing return value")
                    .with_code("E0208")
                    .with_primary(span, format!("expected a `{ret}` value"));
                if let Some(ret_span) = ctx.ret_span {
                    diag = diag.with_secondary(ret_span, "return type declared here");
                }
                self.diags.push(diag);
            }
            (Some(value), Type::Void) => {
                let mut diag = Diagnostic::error(
                    "cannot return a value from a function without a return type",
                )
                .with_code("E0208")
                .with_primary(value.span, "unexpected return value");
                if !matches!(value.ty, Type::Void | Type::Error) {
                    diag = diag.with_help(format!(
                        "declare the return type with `-> {}` after the parameter list",
                        self.show(value.ty)
                    ));
                }
                self.diags.push(diag);
            }
            (Some(value), ret) => {
                self.expect_type(value, ret, ctx.ret_span, "expected due to this return type");
            }
        }
    }

    /// Reports a mismatch if `expr` does not have type `expected`.
    fn expect_type(
        &mut self,
        expr: &CheckedExpr,
        expected: Type,
        reason: Option<Span>,
        reason_label: &str,
    ) {
        if expr.ty == expected || expr.ty == Type::Error || expected == Type::Error {
            return;
        }
        let mut diag = Diagnostic::error("mismatched types")
            .with_code("E0206")
            .with_primary(
                expr.span,
                format!(
                    "expected `{}`, found `{}`",
                    self.show(expected),
                    self.show(expr.ty)
                ),
            );
        if let Some(span) = reason {
            diag = diag.with_secondary(span, reason_label);
        }
        if expr.ty == Type::Void {
            diag = diag.with_note("this expression calls a function that does not return a value");
        } else {
            diag = diag.with_note("Brim never converts between types implicitly");
        }
        diag = match (expected, expr.ty, &expr.kind) {
            (Type::F64, Type::I64, CheckedExprKind::Int(v)) => {
                diag.with_help(format!("write `{v}.0` for a floating-point number"))
            }
            (Type::F64, Type::I64, _) => diag.with_help("convert it with `to_float(...)`"),
            (Type::I64, Type::F64, _) => {
                diag.with_help("convert it with `to_int(...)`, which rounds toward zero")
            }
            (_, found, _)
                if self
                    .fn_type_parts(found)
                    .is_some_and(|(_, ret)| ret == expected) =>
            {
                diag.with_help("this is a function: call it with `(...)` for its result")
            }
            _ => diag,
        };
        self.diags.push(diag);
    }
}
