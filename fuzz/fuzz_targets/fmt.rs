//! The formatter accepts exactly the programs that parse, keeps their
//! tokens (it checks that itself) and is idempotent.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    let (_, diags) = brim_parser::parse(src);
    let parses = !brim_diagnostics::has_errors(&diags);
    match brim_fmt::format(src) {
        Ok(formatted) => {
            assert!(parses, "formatted a program with syntax errors");
            assert_eq!(
                brim_fmt::format(&formatted).expect("formatted text formats"),
                formatted,
                "formatting is idempotent"
            );
        }
        Err(diags) => assert!(!parses, "rejected a valid program: {diags:?}"),
    }
});
