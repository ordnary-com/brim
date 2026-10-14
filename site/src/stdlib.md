# Standard library

Everything here is always available: there is nothing to import. The
[language specification](docs/language-spec.md) (sections 5.4 to 5.6 and
10) is the precise reference.

## Output and input

| Function | Result | |
|----------|--------|---|
| `print(x)` | | writes `x` and a newline to standard output; `x` is an `i64`, `bool` or `string` |
| `eprint(text)` | | writes `text` and a newline to standard error |
| `read_line()` | `Option[string]` | the next line of standard input, without its line ending; `None` at the end |
| `args()` | `List[string]` | the program's command-line arguments, without the program name |
| `env(name)` | `Option[string]` | an environment variable |
| `exit(code)` | | ends the program with an exit code |

```brim
fn main() {
    print("to standard output")
    eprint("to standard error")
    print(42)
    print(1 < 2)
    match env("BRIM_SURELY_UNSET") {
        Some(value) => print("set to " + value)
        None => print("not set")
    }
    print(args().len())
}
```

```text
to standard output
42
true
not set
0
```

A program that reads its input line by line:

```brim
fn main() {
    let mut count: i64 = 0
    while true {
        match read_line() {
            Some(line) => count += 1
            None => break
        }
    }
    print(to_string(count) + " lines")
}
```

## Files

| Function | Result | |
|----------|--------|---|
| `read_file(path)` | `Result[string, string]` | the file's contents |
| `write_file(path, text)` | `Result[i64, string]` | creates or replaces the file; the number of bytes written |

Errors are messages such as `cannot read file "notes.txt": file not found`.
Relative paths are relative to the current directory.

```brim
fn main() {
    match read_file("does-not-exist.txt") {
        Ok(text) => print(text)
        Err(message) => print(message)
    }
}
```

```text
cannot read file "does-not-exist.txt": file not found
```

## Numbers and conversion

| Function | Result | |
|----------|--------|---|
| `to_string(x)` | `string` | an `i64` in decimal, or a `bool` as `"true"` or `"false"` |
| `s.parse_int()` | `Option[i64]` | a decimal integer with an optional sign, and nothing else |

Arithmetic on `i64` is checked: overflow, and division or remainder by
zero, stop the program with a run-time error.

```brim
fn main() {
    print(to_string(7) + to_string(true))
    match "-42".parse_int() {
        Some(n) => print(n * 2)
        None => print("not a number")
    }
    match "4x2".parse_int() {
        Some(n) => print(n)
        None => print("not a number")
    }
}
```

```text
7true
-84
not a number
```

## Strings

Positions and lengths count bytes of the UTF-8 text. Strings compare with
`==`, `!=`, `<`, `<=`, `>` and `>=`, and join with `+`.

| Method | Result | |
|--------|--------|---|
| `len()` | `i64` | the length in bytes |
| `is_empty()` | `bool` | |
| `contains(t)`, `starts_with(t)`, `ends_with(t)` | `bool` | |
| `find(t)` | `Option[i64]` | the position of the first `t` |
| `slice(start, end)` | `string` | bytes `start` up to `end` |
| `trim()`, `trim_start()`, `trim_end()` | `string` | without surrounding whitespace |
| `to_upper()`, `to_lower()` | `string` | ASCII letters only |
| `replace(from, to)` | `string` | every occurrence |
| `repeat(n)` | `string` | |
| `split(sep)` | `List[string]` | an empty separator splits into characters |
| `lines()` | `List[string]` | without line endings |
| `chars()` | `List[string]` | each character as a string |
| `parse_int()` | `Option[i64]` | |

```brim
fn main() {
    let s: string = "  Hello, Brim  "
    let t: string = s.trim()
    print(t.len())
    print(t.to_lower())
    print(t.replace("Brim", "world"))
    print(t.slice(0, 5))
    match t.find(",") {
        Some(i) => print(i)
        None => print("no comma")
    }
    print("ab".repeat(3))
    print("a,b,,c".split(",").len())
    print("x\ny\n".lines().len())
    print("héllo".chars().join("-"))
    print("apple" < "banana")
}
```

```text
11
hello, brim
Hello, world
Hello
5
ababab
4
2
h-é-l-l-o
true
```

## Lists

`List[T]` holds values of one type, in order. Indexing outside the list
stops the program; `get` returns an `Option` instead.

| Method | Result | |
|--------|--------|---|
| `len()`, `is_empty()` | `i64`, `bool` | |
| `get(i)` | `Option[T]` | |
| `first()`, `last()` | `Option[T]` | |
| `contains(x)` | `bool` | for lists of `i64`, `bool` and `string` |
| `join(sep)` | `string` | for `List[string]` |
| `push(x)` | | appends |
| `pop()` | `Option[T]` | removes the last element |
| `insert(i, x)` | | inserts before index `i` |
| `remove(i)` | `T` | removes element `i` |
| `clear()` | | |

`push` and the methods after it change the list, so it needs `let mut`.

```brim
fn main() {
    let mut xs: List[i64] = [3, 1, 4]
    xs.push(1)
    xs.insert(0, 9)
    print(xs.len())
    print(xs[0] + xs[4])
    print(xs.contains(4))
    print(xs.remove(1))
    match xs.pop() {
        Some(x) => print(x)
        None => print("empty")
    }
    match xs.get(10) {
        Some(x) => print(x)
        None => print("no element 10")
    }
    let words: List[string] = ["a", "b", "c"]
    print(words.join("+"))
}
```

```text
5
10
true
3
1
no element 10
a+b+c
```

## Maps

`Map[K, V]` looks up values by key; keys are `i64`, `bool` or `string`.
Entries keep the order in which their keys were first added.

| Method | Result | |
|--------|--------|---|
| `len()`, `is_empty()` | `i64`, `bool` | |
| `get(k)` | `Option[V]` | |
| `contains_key(k)` | `bool` | |
| `keys()`, `values()` | `List[K]`, `List[V]` | in entry order |
| `insert(k, v)` | | adds or replaces |
| `remove(k)` | `Option[V]` | |
| `clear()` | | |

```brim
fn main() {
    let mut stock: Map[string, i64] = ["apples": 3]
    stock["pears"] = 5
    stock["apples"] += 2
    stock.insert("plums", 0)
    for name in stock.keys() {
        print(name + ": " + to_string(stock[name]))
    }
    match stock.remove("plums") {
        Some(n) => print("removed " + to_string(n))
        None => print("no plums")
    }
    print(stock.contains_key("plums"))
}
```

```text
apples: 5
pears: 5
plums: 0
removed 0
false
```

## Option and Result

`Option[T]` is a value that may be absent (`Some(x)` or `None`);
`Result[T, E]` is the outcome of something that can fail (`Ok(x)` or
`Err(e)`). Inspect them with `match`, or pass a failure on with `?`.

```brim
fn half(n: i64) -> Result[i64, string] {
    if n % 2 != 0 {
        return Err(to_string(n) + " is odd")
    }
    return Ok(n / 2)
}

fn quarter(n: i64) -> Result[i64, string] {
    return half(half(n)?)
}

fn main() {
    match quarter(12) {
        Ok(n) => print(n)
        Err(e) => print("error: " + e)
    }
    match quarter(6) {
        Ok(n) => print(n)
        Err(e) => print("error: " + e)
    }
}
```

```text
3
error: 3 is odd
```

## Tests

| Function | |
|----------|---|
| `assert(condition)` | stops the program when the condition is false |
| `assert_eq(a, b)` | stops the program when the values differ, and shows both; for `i64`, `bool` and `string` |

`brim test` runs every function whose name starts with `test_`; see
[Projects, tests and dependencies](projects.md).
