//! Checked programs run in the interpreter without panicking (run-time
//! errors are fine), within a step limit.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    let (ast, diags) = brim_parser::parse(src);
    if brim_diagnostics::has_errors(&diags) {
        return;
    }
    let (checked, _) = brim_semantic::check(&ast);
    let Some(checked) = checked else { return };
    let module = brim_lower::lower(&checked);
    brim_ir::validate(&module).expect("a checked program lowers to valid IR");
    let dir = std::env::temp_dir().join("brim-fuzz-run");
    let _ = std::fs::create_dir_all(&dir);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut host = brim_interp::Host {
        out: &mut out,
        err: &mut err,
        input: &mut std::io::empty(),
        args: Vec::new(),
        cwd: dir,
    };
    let limits = brim_interp::Limits {
        max_call_depth: 200,
        max_steps: Some(50_000),
    };
    let _ = brim_interp::run(&module, &mut host, limits);
});
