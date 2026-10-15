# A tour of Brim

Every example on this page is a complete program; the block under it is
its output. The [language specification](docs/language-spec.md) has
the details.

## Values and variables

```brim
fn main() {
    let name: string = "Brim"
    let year: i64 = 2026
    let ready: bool = true
    print(name + " " + to_string(year))
    print(ready)

    let mut count: i64 = 0
    count += 5
    count = count * 2
    print(count)
}
```

```text
Brim 2026
true
10
```

Variables are immutable unless declared with `let mut`, and always have a
type. The basic types are `i64`, `f64`, `bool` and `string`. Integer
arithmetic is checked: an overflow stops the program with a run-time error
instead of wrapping around.

## Numbers

```brim
fn main() {
    let price: f64 = 19.95
    let count: i64 = 3
    let total: f64 = price * to_float(count)
    print(total)
    print(0.1 + 0.2)
    print(to_int(total))
    match "2.5e3".parse_float() {
        Some(x) => print(x / 1000.0)
        None => print("not a number")
    }
}
```

```text
59.849999999999994
0.30000000000000004
59
2.5
```

`i64` is for counting and `f64` for measuring. They never mix by
accident: `price * count` is an error until you convert one with
`to_float` or `to_int`. `f64` follows IEEE 754, so `0.1 + 0.2` is not
exactly `0.3`, and `1.0 / 0.0` is `inf` rather than an error.

For bytes and other fixed sizes there are `u8`, `i32` and `u64`. A
literal takes the type it needs, and a result that does not fit stops the
program instead of wrapping around:

```brim
fn main() {
    let data: List[u8] = "Hé".bytes()
    print(data)
    let mut level: u8 = 250
    level += 5
    print(level)
    let big: u64 = 18446744073709551615
    print(big / 3)
    print(to_i32(level) - 300)
    print(string_from_bytes(data))
}
```

```text
[72, 195, 169]
255
6148914691236517205
-45
Some("Hé")
```

## Functions

```brim
fn area(width: i64, height: i64) -> i64 {
    return width * height
}

fn greet(name: string) {
    print("Hello, " + name)
}

fn main() {
    greet("Ada")
    print(area(3, 4))
}
```

```text
Hello, Ada
12
```

Functions without `->` return nothing. A function can be used before its
declaration. Statements end at the end of the line.

## Functions as values

```brim
fn apply(f: fn(i64) -> i64, x: i64) -> i64 {
    return f(x)
}

fn double(x: i64) -> i64 {
    return x * 2
}

fn main() {
    print(apply(double, 21))
    let offset: i64 = 100
    let shift: fn(i64) -> i64 = fn(x: i64) -> i64 {
        return x + offset
    }
    print(apply(shift, 1))
}
```

```text
42
101
```

`fn(i64) -> i64` is a function type. A `fn` expression copies the
variables it uses when it is made, so it never changes them.

## Control flow

```brim
fn main() {
    let n: i64 = 7
    if n % 2 == 0 {
        print("even")
    } else if n > 5 {
        print("odd and large")
    } else {
        print("odd")
    }

    let mut i: i64 = 0
    while i < 3 {
        i += 1
    }
    print(i)

    for k in 1..4 {
        print(k * k)
    }
}
```

```text
odd and large
3
1
4
9
```

`for` runs over a range `a..b` (without `b`) or over a list. `break` and
`continue` work in both kinds of loop.

## Structs

```brim
struct Point {
    x: i64,
    y: i64,
}

fn main() {
    let a: Point = Point { x: 1, y: 2 }
    let mut b: Point = a
    b.x = 10
    print(a.x)
    print(b.x + b.y)
}
```

```text
1
12
```

Structs are values: `let mut b = a` copies `a`, so changing `b` leaves `a`
alone.

`==` compares structs, enums, lists and maps by their contents, and `print`
shows them:

```brim
struct Point {
    x: i64,
    y: i64,
}

fn main() {
    let a: Point = Point { x: 1, y: 2 }
    print(a == Point { x: 1, y: 2 })
    print([a, Point { x: 3, y: 4 }])
    print(Some("ada"))
}
```

```text
true
[Point { x: 1, y: 2 }, Point { x: 3, y: 4 }]
Some("ada")
```

## Enums and match

```brim
enum Shape {
    Circle(i64),
    Rect(i64, i64),
    Empty,
}

fn describe(s: Shape) -> string {
    match s {
        Shape.Circle(r) => return "circle with radius " + to_string(r)
        Shape.Rect(w, h) => return to_string(w) + " by " + to_string(h)
        Shape.Empty => return "nothing"
    }
}

fn main() {
    print(describe(Shape.Circle(2)))
    print(describe(Shape.Rect(3, 4)))
    print(describe(Shape.Empty))
}
```

```text
circle with radius 2
3 by 4
nothing
```

`match` must cover every variant; `_` matches the rest. It also works on
integers, strings and booleans.

Patterns nest, `|` joins alternatives, and `if` adds a condition to an arm:

```brim
fn classify(o: Option[i64]) -> string {
    match o {
        Some(0) => return "zero"
        Some(1) | Some(2) | Some(3) => return "small"
        Some(n) if n < 0 => return "negative"
        Some(n) => return "large: " + to_string(n)
        None => return "nothing"
    }
}

fn main() {
    print(classify(Some(2)))
    print(classify(Some(-5)))
    print(classify(Some(40)))
    print(classify(None))
}
```

```text
small
negative
large: 40
nothing
```

A lowercase name in a pattern binds the value; a guard that is false moves
on to the next arm.

An enum can contain itself through a `Box`, which keeps the value on the heap:

```brim
enum Expr {
    Num(i64),
    Add(Box[Expr], Box[Expr]),
}

fn eval(e: Expr) -> i64 {
    match e {
        Num(n) => return n
        Add(a, b) => return eval(a.value()) + eval(b.value())
    }
}

fn main() {
    let e: Expr = Expr.Add(Box(Expr.Num(2)), Box(Expr.Num(3)))
    print(e)
    print(eval(e))
}
```

```text
Add(Num(2), Num(3))
5
```

## Lists and maps

```brim
fn main() {
    let mut scores: List[i64] = [7, 3, 9]
    scores.push(5)
    let mut total: i64 = 0
    for s in scores {
        total += s
    }
    print(to_string(scores.len()) + " scores, total " + to_string(total))
    print(scores[2])

    let mut ages: Map[string, i64] = ["Ada": 36]
    ages["Grace"] = 85
    for name in ages.keys() {
        print(name + ": " + to_string(ages[name]))
    }
    print(ages.contains_key("Linus"))
}
```

```text
4 scores, total 24
9
Ada: 36
Grace: 85
false
```

Lists and maps are values too. Indexing outside a list, or with a key a map
does not have, stops the program; `get` returns an `Option` instead.

## Methods

```brim
struct Counter {
    count: i64,
}

impl Counter {
    fn new() -> Counter {
        return Counter { count: 0 }
    }

    fn add(mut self, n: i64) {
        self.count += n
    }

    fn twice(self) -> i64 {
        return self.count * 2
    }
}

fn main() {
    let mut c: Counter = Counter.new()
    c.add(3)
    c.add(4)
    print(c.twice())
}
```

```text
14
```

A method with `mut self` changes the value it is called on; one with `self`
gets a copy. Methods without `self` are called on the type.

## Generics

```brim
struct Pair[A, B] {
    first: A,
    second: B,
}

fn swap[A, B](p: Pair[A, B]) -> Pair[B, A] {
    return Pair { first: p.second, second: p.first }
}

fn main() {
    let p: Pair[string, i64] = Pair { first: "answer", second: 42 }
    let q: Pair[i64, string] = swap(p)
    print(q.first)
    print(q.second)
}
```

```text
42
answer
```

Type arguments are inferred from the arguments and the expected type.

## Traits

```brim
trait Shape {
    fn area(self) -> f64
}

struct Square {
    side: f64,
}

impl Shape for Square {
    fn area(self) -> f64 {
        return self.side * self.side
    }
}

fn largest[T: Shape](shapes: List[T]) -> f64 {
    let mut best: f64 = 0.0
    for s in shapes {
        if s.area() > best {
            best = s.area()
        }
    }
    return best
}

fn count[T: Eq](xs: List[T], x: T) -> i64 {
    let mut n: i64 = 0
    for y in xs {
        if y == x {
            n += 1
        }
    }
    return n
}

fn main() {
    print(largest([Square { side: 2.0 }, Square { side: 3.0 }]))
    print(count(["a", "b", "a"], "a"))
}
```

```text
9.0
2
```

A bound such as `T: Shape` lets generic code call the trait's methods.
The built-in traits `Eq` and `Show` allow `==` and `print` on a `T`.

## Errors: Option, Result and ?

```brim
fn parse_age(text: string) -> Result[i64, string] {
    match text.parse_int() {
        Some(n) => {
            if n < 0 {
                return Err("an age cannot be negative")
            }
            return Ok(n)
        }
        None => return Err("not a number: " + text)
    }
}

fn total_age(a: string, b: string) -> Result[i64, string] {
    return Ok(parse_age(a)? + parse_age(b)?)
}

fn main() {
    match total_age("36", "85") {
        Ok(n) => print(n)
        Err(e) => print("error: " + e)
    }
    match total_age("36", "old") {
        Ok(n) => print(n)
        Err(e) => print("error: " + e)
    }
}
```

```text
121
error: not a number: old
```

`?` takes the value out of an `Ok` or `Some`, or returns the `Err` or
`None` from the function at once.

## Strings and the standard library

An f-string puts values in text:

```brim
fn main() {
    let name = "Ada"
    let langs = ["en", "fr"]
    print(f"{name} speaks {langs.len()} languages: {langs}")
}
```

```text
Ada speaks 2 languages: ["en", "fr"]
```

```brim
fn main() {
    let line: string = "  Brim is small, Brim is fast  "
    let text: string = line.trim()
    print(text.to_upper())
    print(text.replace("Brim", "it"))
    for word in text.split(", ") {
        print(word.len())
    }
    print(text.starts_with("Brim"))
}
```

```text
BRIM IS SMALL, BRIM IS FAST
it is small, it is fast
13
12
true
```

The standard library also reads input (`read_line`), files (`read_file`,
`write_file`), arguments (`args`) and environment variables (`env`), and
sets the exit code (`exit`).

## Modules

A program can be split into files. Every file is a module; `pub` makes an
item visible to other modules:

```text
src/main.brim
src/geometry.brim
```

```brim
// src/geometry.brim
pub fn square(side: i64) -> i64 {
    return side * side
}
```

```brim
// src/main.brim
import geometry

fn main() {
    print(geometry.square(4))
}
```

The next chapter covers projects, tests and dependencies.
