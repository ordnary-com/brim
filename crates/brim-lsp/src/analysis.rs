//! Analysing a program for the editor: diagnostics, symbols and the
//! declarations names refer to.
//!
//! The queries work on the syntax trees, so they keep working while the
//! program has type errors. Brim requires type annotations on variables and
//! parameters, so their declarations show their types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brim_ast::{Block, FnDecl, ForIter, Ident, Program, Stmt, StmtKind};
use brim_diagnostics::{Diagnostic, SourceFile, SourceMap, Sources, Span};
use brim_driver::Loaded;
use brim_lexer::TokenKind;

/// A checked program, with the files the editor has open.
pub(crate) struct Analysis {
    pub loaded: Loaded,
    /// Syntax, loading and semantic diagnostics; spans are global offsets
    /// into `loaded.map`.
    pub diags: Vec<Diagnostic>,
    /// What names refer to and the types of expressions, from the checker;
    /// empty when the program has syntax errors.
    pub index: brim_semantic::checked::EditorIndex,
}

/// Analyses the program `path` belongs to: the project of the nearest
/// `.brimproj`, starting at its entry file, or the file alone. `open` holds
/// the texts of the files open in the editor.
pub(crate) fn analyze(path: &Path, open: &HashMap<PathBuf, String>) -> Analysis {
    let mut read = |shown: &str| -> std::io::Result<Vec<u8>> {
        let p = PathBuf::from(shown);
        match open.get(&p) {
            Some(text) => Ok(text.clone().into_bytes()),
            None => std::fs::read(&p),
        }
    };
    let project = find_project(path);
    if let Some((graph, root, is_lib)) = &project
        && let Ok(bytes) = read(&root.display().to_string())
        && let Ok(text) = String::from_utf8(bytes)
    {
        let file = SourceFile::new(root.display().to_string(), text);
        let loaded = brim_driver::load(&file, Some(graph), &mut read);
        if loaded
            .map
            .files()
            .iter()
            .any(|f| Path::new(f.name()) == path)
        {
            return check(loaded, *is_lib);
        }
    }
    // A file the project does not use (yet): on its own.
    let text = read(&path.display().to_string())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let file = SourceFile::new(path.display().to_string(), text);
    let graph = project.map(|(graph, _, _)| graph);
    let loaded = brim_driver::load(&file, graph.as_ref(), &mut read);
    check(loaded, true)
}

/// The project of `path`: its packages, its root file and whether that is a
/// library root.
fn find_project(path: &Path) -> Option<(brim_package::Graph, PathBuf, bool)> {
    let manifest_path = brim_manifest::find(path.parent()?)?;
    let dir = manifest_path.parent()?.to_path_buf();
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let file = SourceFile::new(manifest_path.display().to_string(), text);
    let manifest = brim_manifest::parse(file.text()).ok()?;
    let entry = dir.join(&manifest.build.entry);
    let lib = dir.join(&manifest.build.lib);
    let (root, is_lib) = if entry.is_file() || !lib.is_file() {
        (entry, false)
    } else {
        (lib, true)
    };
    let options = brim_package::Options {
        cwd: dir.clone(),
        home: brim_package::default_home()?,
        update: false,
        registry: None,
        // Opening a file must not clone, download or write anything.
        offline: true,
    };
    let (graph, _) = brim_package::resolve(&dir, manifest, file, &options).ok()?;
    Some((graph, root, is_lib))
}

fn check(loaded: Loaded, main_optional: bool) -> Analysis {
    let mut diags = loaded.diags.clone();
    let mut index = brim_semantic::checked::EditorIndex::default();
    if !brim_diagnostics::has_errors(&diags) {
        let (found, semantic) = brim_semantic::index_modules(&loaded.modules, main_optional);
        index = found;
        diags.extend(semantic);
    }
    Analysis {
        loaded,
        diags,
        index,
    }
}

/// A declaration a name refers to.
pub(crate) struct Declaration {
    /// The declared name.
    pub span: Span,
    /// The declaration as shown on hover.
    pub detail: String,
}

impl Analysis {
    /// The index of the file (and module) named `path`.
    pub fn file_index(&self, path: &Path) -> Option<usize> {
        self.loaded
            .map
            .files()
            .iter()
            .position(|f| Path::new(f.name()) == path)
    }

    pub fn map(&self) -> &SourceMap {
        &self.loaded.map
    }

    fn text(&self, span: Span) -> &str {
        let (file, base, _) = self.loaded.map.locate(span.start);
        &file.text()[span.start - base..span.end - base]
    }

    /// The declarations of a module, for the outline.
    pub fn symbols(&self, module: usize) -> Vec<Symbol> {
        let program = &self.loaded.modules[module].program;
        let mut out = Vec::new();
        for s in &program.structs {
            out.push(Symbol {
                name: s.name.name.clone(),
                kind: 23,
                span: s.span,
                name_span: s.name.span,
                children: s
                    .fields
                    .iter()
                    .map(|f| Symbol {
                        name: f.name.name.clone(),
                        kind: 8,
                        span: f.span,
                        name_span: f.name.span,
                        children: Vec::new(),
                    })
                    .collect(),
            });
        }
        for e in &program.enums {
            out.push(Symbol {
                name: e.name.name.clone(),
                kind: 10,
                span: e.span,
                name_span: e.name.span,
                children: e
                    .variants
                    .iter()
                    .map(|v| Symbol {
                        name: v.name.name.clone(),
                        kind: 22,
                        span: v.span,
                        name_span: v.name.span,
                        children: Vec::new(),
                    })
                    .collect(),
            });
        }
        for i in &program.impls {
            out.push(Symbol {
                name: format!("impl {}", i.type_name.name),
                kind: 5,
                span: i.span,
                name_span: i.type_name.span,
                children: i.methods.iter().map(|f| fn_symbol(f, 6)).collect(),
            });
        }
        for f in &program.functions {
            out.push(fn_symbol(f, 12));
        }
        out.sort_by_key(|s| s.span.start);
        out
    }

    /// What the name at global `offset` in `module` refers to.
    pub fn declarations(&self, module: usize, offset: usize) -> Vec<Declaration> {
        let map = &self.loaded.map;
        let (file, base, _) = map.locate(offset);
        let (tokens, _) = brim_lexer::lex(file.text());
        let local = offset - base;
        let Some(at) = tokens
            .iter()
            .position(|t| t.span.start <= local && local <= t.span.end && is_name(&t.kind))
        else {
            return Vec::new();
        };
        let name = file.text()[tokens[at].span.start..tokens[at].span.end].to_string();
        let prev = |n: usize| at.checked_sub(n).map(|i| &tokens[i].kind);
        let program = &self.loaded.modules[module].program;

        // `base.name`
        if prev(1) == Some(&TokenKind::Dot) {
            if let Some(TokenKind::Ident(base_name)) = prev(2) {
                if let Some(target) = self.module_alias(program, base_name) {
                    return self.items(target, &name);
                }
                let mut found = self.type_members(base_name, &name);
                if !found.is_empty() {
                    return found;
                }
                found = self.members(&name);
                return found;
            }
            return self.members(&name);
        }

        if name == "self" {
            return self.enclosing_impl(program, offset);
        }
        if let Some(found) = self.local(program, &name, offset) {
            return vec![found];
        }
        let own = self.items(module, &name);
        if !own.is_empty() {
            return own;
        }
        for import in &program.imports {
            if let Some(items) = &import.items
                && items.iter().any(|i| i.name == name)
                && let Some(target) = self.module_named(&joined(&import.path))
            {
                return self.items(target, &name);
            }
        }
        if let Some(target) = self.module_alias(program, &name) {
            let span = self.module_start(target);
            return vec![Declaration {
                span,
                detail: format!("module {}", self.loaded.modules[target].name),
            }];
        }
        Vec::new()
    }

    pub(crate) fn module_named(&self, name: &str) -> Option<usize> {
        self.loaded.modules.iter().position(|m| m.name == name)
    }

    fn module_start(&self, module: usize) -> Span {
        let base = self.loaded.map.base(module);
        Span::new(base, base)
    }

    /// The module an import makes available under `alias`.
    pub(crate) fn module_alias_index(&self, program: &Program, alias: &str) -> Option<usize> {
        self.module_alias(program, alias)
    }

    /// The module an import makes available under `alias`.
    fn module_alias(&self, program: &Program, alias: &str) -> Option<usize> {
        program.imports.iter().find_map(|import| {
            if import.items.is_some() {
                return None;
            }
            let name = import.alias.as_ref().or(import.path.last())?;
            (name.name == alias)
                .then(|| self.module_named(&joined(&import.path)))
                .flatten()
        })
    }

    /// The top-level items called `name` in a module.
    fn items(&self, module: usize, name: &str) -> Vec<Declaration> {
        let program = &self.loaded.modules[module].program;
        let mut out = Vec::new();
        for f in program.functions.iter().filter(|f| f.name.name == name) {
            out.push(self.fn_declaration(f));
        }
        for s in program.structs.iter().filter(|s| s.name.name == name) {
            out.push(Declaration {
                span: s.name.span,
                detail: with_pub(s.public, self.text(s.span)),
            });
        }
        for e in program.enums.iter().filter(|e| e.name.name == name) {
            out.push(Declaration {
                span: e.name.span,
                detail: with_pub(e.public, self.text(e.span)),
            });
        }
        out
    }

    /// Variants and methods of the type `type_name`.
    fn type_members(&self, type_name: &str, name: &str) -> Vec<Declaration> {
        let mut out = Vec::new();
        for module in &self.loaded.modules {
            for e in module
                .program
                .enums
                .iter()
                .filter(|e| e.name.name == type_name)
            {
                for v in e.variants.iter().filter(|v| v.name.name == name) {
                    out.push(Declaration {
                        span: v.name.span,
                        detail: format!("{type_name}.{}", self.text(v.span)),
                    });
                }
            }
            for i in module
                .program
                .impls
                .iter()
                .filter(|i| i.type_name.name == type_name)
            {
                for f in i.methods.iter().filter(|f| f.name.name == name) {
                    out.push(self.fn_declaration(f));
                }
            }
        }
        out
    }

    /// Fields and methods called `name` of any type.
    fn members(&self, name: &str) -> Vec<Declaration> {
        let mut out = Vec::new();
        for module in &self.loaded.modules {
            for s in &module.program.structs {
                for f in s.fields.iter().filter(|f| f.name.name == name) {
                    out.push(Declaration {
                        span: f.name.span,
                        detail: format!("{}.{}", s.name.name, self.text(f.span)),
                    });
                }
            }
            for i in &module.program.impls {
                for f in i.methods.iter().filter(|f| f.name.name == name) {
                    out.push(self.fn_declaration(f));
                }
            }
        }
        out
    }

    fn enclosing_impl(&self, program: &Program, offset: usize) -> Vec<Declaration> {
        program
            .impls
            .iter()
            .filter(|i| i.span.start <= offset && offset <= i.span.end)
            .flat_map(|i| self.type_members_decl(&i.type_name.name))
            .collect()
    }

    fn type_members_decl(&self, type_name: &str) -> Vec<Declaration> {
        let mut out = Vec::new();
        for module in &self.loaded.modules {
            for s in module
                .program
                .structs
                .iter()
                .filter(|s| s.name.name == type_name)
            {
                out.push(Declaration {
                    span: s.name.span,
                    detail: with_pub(s.public, self.text(s.span)),
                });
            }
            for e in module
                .program
                .enums
                .iter()
                .filter(|e| e.name.name == type_name)
            {
                out.push(Declaration {
                    span: e.name.span,
                    detail: with_pub(e.public, self.text(e.span)),
                });
            }
        }
        out
    }

    fn fn_declaration(&self, f: &FnDecl) -> Declaration {
        let header = Span::new(f.span.start, f.body.span.start);
        Declaration {
            span: f.name.span,
            detail: with_pub(f.public, self.text(header).trim_end()),
        }
    }

    /// The variables and parameters declared before `offset` in the
    /// function around it, with how they are declared; the last one of each
    /// name.
    pub(crate) fn local_names(&self, program: &Program, offset: usize) -> Vec<(String, String)> {
        let functions = program
            .functions
            .iter()
            .chain(program.impls.iter().flat_map(|i| &i.methods));
        let Some(f) = functions
            .into_iter()
            .find(|f| f.span.start <= offset && offset <= f.span.end)
        else {
            return Vec::new();
        };
        let mut found: Vec<(String, String)> = Vec::new();
        let mut consider = |ident: &Ident, detail: String| {
            if ident.span.end <= offset {
                found.retain(|(n, _)| *n != ident.name);
                found.push((ident.name.clone(), detail));
            }
        };
        for p in &f.params {
            consider(&p.name, self.text(p.span).to_string());
        }
        self.block_locals(&f.body, &mut consider);
        found
    }

    /// The variable or parameter `name` visible at `offset`: the last one
    /// declared before it in the enclosing function.
    fn local(&self, program: &Program, name: &str, offset: usize) -> Option<Declaration> {
        let functions = program
            .functions
            .iter()
            .chain(program.impls.iter().flat_map(|i| &i.methods));
        let f = functions
            .into_iter()
            .find(|f| f.span.start <= offset && offset <= f.span.end)?;
        let mut found: Option<Declaration> = None;
        let mut consider = |ident: &Ident, detail: String| {
            if ident.name == name && ident.span.start <= offset {
                found = Some(Declaration {
                    span: ident.span,
                    detail,
                });
            }
        };
        for p in &f.params {
            consider(&p.name, self.text(p.span).to_string());
        }
        self.block_locals(&f.body, &mut consider);
        found
    }

    fn block_locals(&self, block: &Block, consider: &mut dyn FnMut(&Ident, String)) {
        for stmt in &block.stmts {
            self.stmt_locals(stmt, consider);
        }
    }

    fn stmt_locals(&self, stmt: &Stmt, consider: &mut dyn FnMut(&Ident, String)) {
        match &stmt.kind {
            StmtKind::LetPattern { pattern, .. } => {
                pattern.for_each_binding(&mut |b| {
                    consider(b, format!("let {} in {}", b.name, self.text(pattern.span)));
                });
            }
            StmtKind::Let {
                mutable, name, ty, ..
            } => {
                let m = if *mutable { "mut " } else { "" };
                let ty = ty
                    .as_ref()
                    .map_or(String::new(), |ty| format!(": {}", self.text(ty.span)));
                consider(name, format!("let {m}{}{ty}", name.name));
            }
            StmtKind::If {
                then_block,
                else_branch,
                ..
            } => {
                self.block_locals(then_block, consider);
                if let Some(e) = else_branch {
                    self.stmt_locals(e, consider);
                }
            }
            StmtKind::While { body, .. } => self.block_locals(body, consider),
            StmtKind::For { var, iter, body } => {
                let what = match iter {
                    ForIter::Each(e) => format!("for {} in {}", var.name, self.text(e.span)),
                    ForIter::Range(a, b) => format!(
                        "for {}: i64 in {}..{}",
                        var.name,
                        self.text(a.span),
                        self.text(b.span)
                    ),
                };
                consider(var, what);
                self.block_locals(body, consider);
            }
            StmtKind::Match { arms, .. } => {
                for arm in arms {
                    arm.pattern.for_each_binding(&mut |b| {
                        consider(b, format!("{} in {}", b.name, self.text(arm.pattern.span)));
                    });
                    self.block_locals(&arm.body, consider);
                }
            }
            StmtKind::Block(b) => self.block_locals(b, consider),
            StmtKind::Assign { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Return(_)
            | StmtKind::Expr(_) => {}
        }
    }
}

/// An entry of the outline.
pub(crate) struct Symbol {
    pub name: String,
    /// The LSP `SymbolKind`.
    pub kind: u32,
    pub span: Span,
    pub name_span: Span,
    pub children: Vec<Symbol>,
}

fn fn_symbol(f: &FnDecl, kind: u32) -> Symbol {
    Symbol {
        name: f.name.name.clone(),
        kind,
        span: f.span,
        name_span: f.name.span,
        children: Vec::new(),
    }
}

/// A declaration's text; its span does not include `pub`.
fn with_pub(public: bool, text: &str) -> String {
    if public && !text.starts_with("pub ") {
        format!("pub {text}")
    } else {
        text.to_string()
    }
}

fn is_name(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Ident(_) | TokenKind::SelfValue)
}

fn joined(path: &[Ident]) -> String {
    path.iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}
