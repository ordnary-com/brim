# Projects, tests and dependencies

## Projects

A project is a directory with a `.brimproj`:

```toml
[package]
name = "greeter"
version = "0.1.0"
edition = "2026"
```

The program starts in `src/main.brim`; a library's root is `src/lib.brim`.
Other modules live next to them: `use text/format` is
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

Packages can use other packages: from the package registry by version, from
a directory, or from a git repository:

```toml
[dependencies]
router = "1.2"
geometry = { path = "../geometry" }
http = { git = "https://github.com/example/brim-http.git", tag = "v1.0.0" }
```

`use @geometry` then refers to the library root of `geometry`
(`src/lib.brim`), and `use @geometry/shapes` to its `src/shapes.brim`.
The `@` marks a package; a path without it is a module of your own
project, so `use shapes` and `use @geometry/shapes` never get mixed up.
The standard library is the package `@std`.

### Versions

`router = "1.2"` means any version from 1.2.0 up to, but not including,
2.0.0: a newer version that promises to be compatible. Other forms:

| Requirement | Versions |
|-------------|----------|
| `"1.2.3"` or `"^1.2.3"` | 1.2.3 up to 2.0.0 |
| `"0.3"` | 0.3.0 up to 0.4.0 (before 1.0, the second number is the major one) |
| `"~1.2.3"` | 1.2.3 up to 1.3.0 |
| `"=1.2.3"` | exactly 1.2.3 |
| `">=1.2, <1.5"` | the comparisons, all of them |
| `"*"` | any version |

Pre-releases such as `2.0.0-beta.1` are only picked when the requirement
names one. Every package has one version in a project: the newest that
meets all requirements on it, from the project and from other packages.
When there is none, brim says who requires what.

### .brimlock

The picked versions and the commits of git dependencies are recorded in
`.brimlock`, so every build uses the same versions, also without a network;
commit it with your program. `brim update` picks the newest versions and
commits again. Downloads are cached in `~/.brim` (or `$BRIM_HOME`).

### Publishing

```sh
brim login            # paste an API token for the registry
brim publish --dry-run
brim publish
```

`brim publish` checks the package, packs `.brimproj`, `src/` and the
README, LICENSE and CHANGELOG files, and uploads them. Give the package a
`description` and a `license` in `[package]`. Every dependency needs a
version; a path dependency can have one too, which is what the published
package keeps:

```toml
[dependencies]
shared = { path = "../shared", version = "0.1" }
```

A published version never changes. `brim yank router@1.2.0` stops new
projects from picking a broken version; projects whose `.brimlock` has it
keep working.

## Workspaces

A workspace is a directory with packages that are developed together. Its
`.brimproj` lists them:

```toml
[workspace]
members = ["apps/*", "shared"]
```

`apps/*` stands for every directory in `apps` with a `.brimproj`. The
members share one `.brimlock`, at the root, and every dependency has one
version in the whole workspace. At the root, `brim check`, `brim test`,
`brim build` and `brim doc` work on every member; `-p NAME` picks one, as
does running brim in a member's directory. Members depend on each other
with `path`.

## Calling C

An `extern fn` is a C function, declared without a body:

```brim
extern fn crc32(data: string, len: i64) -> u64

fn main() {
    print(crc32("hello", 5))
}
```

The C code comes from your package, the libraries it names, or the C
library:

```toml
[build]
c-sources = ["c/checksum.c"]
link = ["z"]
```

`extern fn`s take and return `i64`, `i32`, `u8`, `u64`, `f64`, `bool` and
`string` (passed as a `const char *`), and may return nothing. They run in
native executables: `brim build`, and `brim run`, which builds such a
program for you. The interpreter cannot call C, so `brim test` stops at a
call with `R0011`.
