# Getting started

## Installing

On macOS or Linux:

```sh
curl -fsSL https://brimlang.com/install.sh | sh
```

On Windows, in PowerShell:

```sh
irm https://brimlang.com/install.ps1 | iex
```

The script picks the build for your platform, checks its checksum and
puts `brim` in `/usr/local/bin` (or `~/.local/bin`). You can also download
an archive from the [releases](https://github.com/ordnary-com/brim/releases)
yourself and put `brim` on your `PATH`. The `-native` archives can also compile native executables
with `brim build`; they need a C compiler to link: `cc` on Linux and
macOS, and on Windows `clang` (from LLVM) with the Visual Studio C++ build
tools.

The scripts also give `.brim` and `.brimproj` files their own icons in
Finder, File Explorer and Linux file managers, with `brim icons`. Files
still open in the editor you use for them. Run `brim icons` yourself after
installing another way (Homebrew, an archive, from source), and
`brim icons --remove` to take the icons away again.

Or build it from source with Rust 1.88 or newer:

```sh
git clone https://github.com/ordnary-com/brim.git
cd brim
cargo install --path crates/brim-cli
```

For native builds from source, see the
[development guide](docs/development.md#native-builds-llvm).

## A first program

Save this as `hello.brim`:

```brim
fn main() {
    print("Hello, world!")
}
```

and run it:

```sh
brim run hello.brim
```

`brim check hello.brim` only reports errors, and `brim build hello.brim`
writes a native executable `hello` next to it.

## A first project

```sh
brim new greeter
cd greeter
brim run
brim test
```

`brim new` creates a `.brimproj`, a `src/main.brim` with a test, and a
`.gitignore`. Inside a project, commands find the `.brimproj` themselves.
Continue with [the tour](tour.md).
