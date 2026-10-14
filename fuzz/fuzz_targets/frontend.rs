//! Lexing, parsing, semantic analysis and lowering never panic, and a
//! checked program lowers to valid IR.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    let (ast, diags) = brim_parser::parse(src);
    let file = brim_diagnostics::SourceFile::new("fuzz.brim", src);
    let renderer = brim_diagnostics::Renderer::new(brim_diagnostics::ColorChoice::Never);
    let _ = renderer.render_all(&diags, &file);
    if brim_diagnostics::has_errors(&diags) {
        return;
    }
    let modules = [brim_semantic::ModuleAst {
        name: String::new(),
        program: ast,
    }];
    let (checked, diags) = brim_semantic::check_tests(&modules);
    let _ = renderer.render_all(&diags, &file);
    if let Some(checked) = checked {
        let (module, _) = brim_lower::lower_tests(&checked);
        brim_ir::validate(&module).expect("a checked program lowers to valid IR");
    }
});
