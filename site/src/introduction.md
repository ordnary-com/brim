# Brim

Brim is a small, statically typed programming language by Ordnary, for
command-line tools and backend services. It compiles to native executables
through LLVM, and comes with one `brim` command for everything else:
running, testing, formatting, managing dependencies and editor support.

```brim
fn main() {
    let names: List[string] = ["Ada", "Grace", "Linus"]
    for name in names {
        print("Hello, " + name + "!")
    }
}
```

```text
Hello, Ada!
Hello, Grace!
Hello, Linus!
```

Brim aims to be:

- **Explicit.** Every parameter, result and field has a declared type, a
  variable has the type of its value, values are never converted implicitly, and errors are values (`Result` with the
  `?` operator), not exceptions.
- **Predictable.** Structs, lists and maps are values: assigning one copies
  it (cheaply, by copy-on-write). Memory is managed by reference counting,
  without a garbage collector.
- **Helpful.** Every diagnostic has a code, points at the source and
  usually says how to fix the problem. Run-time errors show the call stack.

Brim is heading for 1.0, edition 2026: from 1.0 the language, the
standard library and the `brim` command are stable
([Releasing](docs/releasing.md)). The [roadmap](docs/roadmap.md) lists what
comes next.

Start with [Getting started](getting-started.md), or take
[the tour](tour.md).
