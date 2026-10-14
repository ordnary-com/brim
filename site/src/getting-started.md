# Getting started

## Installing

Download an archive for your platform from the
[releases](https://github.com/ordnary-com/brim/releases) and put `brim` on
your `PATH`. The `-native` archives can also compile native executables
with `brim build`; they need a C compiler (`cc`) to link.

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

`brim new` creates a `brim.toml`, a `src/main.brim` with a test, and a
`.gitignore`. Inside a project, commands find the `brim.toml` themselves.
Continue with [the tour](tour.md).
