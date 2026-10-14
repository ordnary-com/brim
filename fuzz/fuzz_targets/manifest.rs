//! Manifest parsing never panics, and its diagnostics point into the text.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|src: &str| {
    if let Err(diags) = brim_manifest::parse(src) {
        for d in diags {
            if let Some(span) = d.span() {
                assert!(span.end <= src.len());
            }
        }
    }
});
