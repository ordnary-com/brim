# Projects, tests and dependencies

## Projects

A project is a directory with a `brim.toml`:

```toml
[package]
name = "greeter"
version = "0.1.0"
edition = "2026"
```

The program starts in `src/main.brim`; a library's root is `src/lib.brim`.
Other modules live next to them: `import text.format` is
`src/text/format.brim`. Inside the project, `brim run`, `brim build`,
`brim check`, `brim test` and `brim fmt` need no path.

## Tests

Functions whose names start with `test_` are tests. `assert` and
`assert_eq` stop a test when something is wrong:

```brim
fn add(a: i64, b: i64) -> i64 {
    return a + b
}

fn test_add() {
    assert_eq(add(2, 3), 5)
    assert(add(-1, 1) == 0)
}
```

`brim test` runs every test of the project and reports each one. A failed
`assert_eq` shows both values:

```text
running 1 test
test test_add ... FAILED

failures:

---- test_add ----
error[R0008]: assertion failed: the values are not equal
 --> src/main.brim:6:5
  |
6 |     assert_eq(add(2, 3), 6)
  |     ^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: left:  5
  = note: right: 6

test result: FAILED. 0 passed; 1 failed
```

`brim test parse` runs only the tests whose names contain `parse`.

## Dependencies

Packages can use other packages, from a directory or a git repository:

```toml
[dependencies]
geometry = { path = "../geometry" }
http = { git = "https://github.com/example/brim-http.git", tag = "v1.0.0" }
```

`import geometry` then refers to the library root of `geometry`
(`src/lib.brim`), and `import geometry.shapes` to its `src/shapes.brim`.

The commits of git dependencies are recorded in `brim.lock`, so every
build uses the same versions; commit it with your program. `brim update`
moves them to the newest commits. Repositories are cached in
`~/.brim/git` (or `$BRIM_HOME/git`).

There is no package registry yet.
