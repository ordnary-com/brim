# Releasing

Brim follows [Semantic Versioning](https://semver.org/) from 1.0. A minor
version (1.x.0) adds features; a patch version (1.x.y) only fixes bugs.
Nothing in 1.x breaks what the next section calls stable. Before 1.0, a
minor version (0.x.0) could change anything.

## Stability

These are stable from 1.0, for every 1.x release:

- **The language**, per edition (below), as `docs/language-spec.md`
  describes it.
- **The standard library**: the `pub` items of the `std` modules, the
  builtins and the prelude, with their behaviour. New items, methods and
  modules may be added. A program's own names win over the prelude's, and
  built-in types cannot get methods in programs, so additions do not clash
  with existing code.
- **The command line**: the commands and options of `brim` and their
  meaning, and its exit codes (see `error-handling.md`).
- **`.brimproj`** and **`.brimlock`**: newer compilers read every earlier
  manifest and lock file.
- **Diagnostic codes** (`E0201`, `W0003`, `R0001`, `M0014`, ...) keep their
  meaning; new codes are added, old ones are not reused.
- **The registry protocol**, version 1 (`docs/registry.md`).
- **Behaviour.** A program prints the same and exits the same way with
  every 1.x compiler, in the interpreter and in native code. The exceptions
  are the order in which tasks run, and so the order of their output, and
  messages: see below.

Not stable: the text of messages (diagnostics, run-time errors and error
strings such as `connection refused` are meant for people and may become
clearer), new warnings, performance, the IR text form, the runtime ABI
between the compiler and its C runtime, and the Rust crates' APIs.

Fixing a bug may make the compiler reject a program it wrongly accepted,
or stop a program that relied on a wrong result. Such fixes are listed
under **Fixed** in the changelog, and where it is possible the compiler
warns in an earlier release first.

## Editions

A package declares its edition in `.brimproj` (`edition = "2026"`). 1.0
has one edition, `2026`.

- Within an edition, a program that compiles keeps compiling and keeps its
  meaning in every later compiler.
- Changes that would break programs, such as a new keyword or a different
  meaning for existing code, wait for a new edition. Editions are named
  after their year and come only when such a change is worth it, at most
  once every three years.
- Every compiler supports every earlier edition, so a package moves to a
  new edition when its authors choose, not when they update `brim`.
- Packages of different editions work together in one program: an edition
  changes how a package's source is read, not its types and functions as
  other packages see them.
- A new edition comes with a migration guide, and with an automatic
  migration in `brim fmt` where the change allows one.
- The reserved words (`async`, `await`, `type`, `use`) are already
  unavailable as names, so features that use them can arrive in a 1.x
  release without a new edition.

## Release binaries

The release workflow (`.github/workflows/release.yml`) builds:

| Archive | Contents |
|---------|----------|
| `brim-X.Y.Z-linux-x86_64`, `-linux-aarch64`, `-macos-aarch64`, `-windows-x86_64` | `brim` with the interpreter and all tools (`run`, `check`, `test`, `fmt`, `gen`, `lsp`, ...), without native builds |
| `brim-X.Y.Z-linux-x86_64-native`, `-macos-aarch64-native`, `-windows-x86_64-native` | the same, with the LLVM backend linked in for `brim build` (which also needs a C compiler: `clang` with the Visual Studio C++ build tools on Windows) |

Each archive holds `brim`, the README, the license and the changelog. The
Linux builds need glibc 2.34 or newer (Ubuntu 22.04, Debian 12, Fedora 35
and later); the native Linux build also uses the system's zlib, zstd and
libstdc++. The macOS builds need nothing beyond macOS itself. `brim` uses the
system's libcurl for HTTPS (`libcurl4` on Debian and Ubuntu, installed
almost everywhere); a native program that makes HTTPS requests is linked
with it too, which on Linux needs its development files
(`libcurl4-openssl-dev`). The
release has a `SHA256SUMS` file. Running the workflow by hand
(`workflow_dispatch`) builds the archives as workflow artifacts without
publishing anything: a dry run.

## Making a release

1. Make sure `main` is green, including the native jobs.
2. Set the version in the workspace `Cargo.toml` (`[workspace.package]
   version`) and run `cargo check` to update `Cargo.lock`.
3. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`
   and start a new, empty `## [Unreleased]` section above it.
4. Update the version in `editors/vscode/package.json`.
5. Commit (`Release X.Y.Z`), tag and push:

   ```sh
   git tag -a vX.Y.Z -m "Brim X.Y.Z"
   git push origin main vX.Y.Z
   ```

6. The workflow publishes the GitHub release with the changelog section as
   notes. Versions below 1.0 and versions with a pre-release suffix
   (`v1.0.0-rc.1`) are marked as pre-releases.
