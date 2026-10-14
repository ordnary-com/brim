# Fuzzing

Coverage-guided fuzz targets for [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz).
They need nightly Rust:

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run frontend fuzz/corpus/frontend -- -max_total_time=300
```

| Target     | Checks |
|------------|--------|
| `frontend` | lexing, parsing, semantic analysis and lowering never panic; checked programs lower to valid IR |
| `fmt`      | the formatter accepts exactly the programs that parse and is idempotent |
| `run`      | checked programs run in the interpreter without panicking, within a step limit |
| `manifest` | `brim.toml` parsing never panics |

Seed a corpus with the example and test programs:

```sh
mkdir -p fuzz/corpus/frontend
find examples tests -name '*.brim' -exec cp {} fuzz/corpus/frontend/ \;
```

A quicker, deterministic mutation fuzzer runs with the normal tests on
stable Rust: `crates/brim-cli/tests/fuzz_smoke.rs`.

A crash found by a target becomes a regression test: add the input to
`tests/` as a program with the expected diagnostics or output.
