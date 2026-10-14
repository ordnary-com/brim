# Releasing

Brim follows [Semantic Versioning](https://semver.org/) once it reaches
1.0. Before 1.0, a minor version (0.x.0) may change the language or the
command line; a patch version (0.x.y) only fixes bugs.

## Stability

- **Editions.** A program declares its edition in `brim.toml`
  (`edition = "2026"`). Within an edition, a program that compiles keeps
  compiling and keeps its meaning in every later compiler, apart from fixes
  to bugs that made the compiler accept wrong programs. Changes that break
  programs wait for a new edition, and a compiler supports every earlier
  edition. Before 1.0 this is a goal, not yet a promise.
- **Diagnostic codes** (`E0201`, `R0001`, ...) keep their meaning; new
  codes are added, old ones are not reused.
- **Exit codes** of `brim` (see `error-handling.md`) are stable.
- **`brim.lock`** has a format version; newer compilers read older lock
  files.
- Not stable: the text of messages, the IR text form, the runtime ABI
  between the compiler and its C runtime, and the Rust crates' APIs.

## Release binaries

The release workflow (`.github/workflows/release.yml`) builds:

| Archive | Contents |
|---------|----------|
| `brim-X.Y.Z-linux-x86_64`, `-linux-aarch64`, `-macos-aarch64`, `-windows-x86_64` | `brim` with the interpreter and all tools (`run`, `check`, `test`, `fmt`, `gen`, `lsp`, ...), without native builds |
| `brim-X.Y.Z-linux-x86_64-native`, `-macos-aarch64-native` | the same, with the LLVM backend linked in for `brim build` (which also needs a C compiler) |

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
