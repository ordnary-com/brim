# Standard library

Everything here is always available: there is nothing to import. The
[language specification](docs/language-spec.md) (sections 5.4 to 5.6 and
10) is the precise reference, and the [API reference](std/index.html),
made with `brim doc --std`, lists every function and type of the `std`
modules with its documentation.

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

### Directories and more: std.fs

`std.fs` has the rest of what a program does with files:

| Function | Result | |
|----------|--------|---|
| `exists(path)`, `is_dir(path)`, `is_file(path)` | `bool` | |
| `list(dir)` | `Result[List[string], string]` | the names in a directory, sorted |
| `create_dir(path)` | `Result[bool, string]` | with any missing parents; `Ok(false)` if it existed |
| `remove(path)` | `Result[bool, string]` | a file or an empty directory; `Ok(false)` if there was none |
| `remove_all(path)` | `Result[bool, string]` | a directory with everything in it |
| `rename(from, to)` | `Result[bool, string]` | also moves |
| `size(path)` | `Result[i64, string]` | in bytes |
| `modified(path)` | `Result[time.Time, string]` | when it last changed |
| `read(path)`, `write(path, text)`, `append(path, text)` | | `read` and `write` are `read_file` and `write_file` |

```brim
use @std/fs

fn main() {
    match fs.list("no-such-dir") {
        Ok(names) => print(names)
        Err(message) => print(message)
    }
}
```

```text
cannot list directory "no-such-dir": not found
```

## Numbers and conversion

| Function | Result | |
|----------|--------|---|
| `to_string(x)` | `string` | a number in decimal, or a `bool` as `"true"` or `"false"` |
| `to_float(n)` | `f64` | an `i64` as the nearest `f64` |
| `to_int(x)` | `i64` | an `f64` rounded toward zero; NaN or a number outside `i64` stops the program |
| `s.parse_int()` | `Option[i64]` | a decimal integer with an optional sign, and nothing else |
| `s.parse_float()` | `Option[f64]` | a decimal number such as `-1.5e3` |

Arithmetic on `i64` is checked: overflow, and division or remainder by
zero, stop the program with a run-time error. Arithmetic on `f64` follows
IEEE 754 and never stops the program: `1.0 / 0.0` is `inf`. An `f64` is
written with the fewest digits that read back exactly (`0.1`, `2.0`,
`1e21`).

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

### Math

| Function | Result | |
|----------|--------|---|
| `abs(x)` | `i64` or `f64` | the absolute value; `abs` of the smallest `i64` stops the program |
| `sqrt(x)`, `floor(x)`, `ceil(x)`, `round(x)` | `f64` | `round` rounds halves away from zero |
| `pow(a, b)` | `i64` or `f64` | `a` to the power `b`; on integers a negative power divides (`pow(2, -1)` is 0) and overflow stops the program |
| `min(a, b)`, `max(a, b)` | the argument type | for numbers and strings |
| `clamp(x, low, high)` | the argument type | `x` moved into the range |
| `PI`, `E` | `f64` | the constants |

```brim
fn main() {
    let r = 2.0
    print(round(PI * r * r))
    print(pow(2, 10))
    print(sqrt(2.0))
    print(clamp(150, 0, 100))
    print(max("pear", "apple"))
}
```

```text
13.0
1024
1.4142135623730951
100
pear
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
| `char_count()` | `i64` | the number of characters |
| `code_points()` | `List[i64]` | the Unicode code point of each character |
| `bytes()` | `List[u8]` | the UTF-8 bytes |
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

`from_code_point(n)` is the character with a code point, or `None`. To
build a long string from many pieces, use a `StringBuilder`: it joins the
pieces once, where `+` in a loop copies the text each time.

```brim
fn main() {
    let mut out = StringBuilder.new()
    for cp in "HAL".code_points() {
        match from_code_point(cp + 1) {
            Some(c) => out.add(c)
            None => out.add("?")
        }
    }
    out.add_line("!")
    print(out.build())
}
```

```text
IBM!

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
| `sorted()`, `reversed()` | `List[T]` | a sorted or reversed copy |
| `sorted_by(less)` | `List[T]` | a copy sorted by `fn(a, b) -> bool` |
| `slice(a, b)` | `List[T]` | elements `a` up to `b`, clamped to the list |
| `index_of(x)` | `Option[i64]` | where `x` is first |
| `map(f)`, `filter(keep)` | `List` | |
| `any(test)`, `all(test)` | `bool` | |
| `fold(init, f)` | | combines the elements, left to right |
| `find(test)`, `min()`, `max()` | `Option[T]` | |
| `sum()` | `T` | for lists of numbers; 0 when empty |
| `each(f)` | | calls `f` with each element |

`push` up to `clear` change the list, so it needs `let mut`. The others
return a new list and leave the original as it was:

```brim
fn main() {
    let xs = [3, 1, 2]
    print(xs.sorted())
    print(xs.reversed().slice(0, 2))
    print(xs.index_of(2))
    print(["bb", "a", "ccc"].sorted_by(fn(a: string, b: string) -> bool {
        return a.len() < b.len()
    }))
}
```

```text
[1, 2, 3]
[2, 1]
Some(2)
["a", "bb", "ccc"]
```

Functions passed to `map`, `filter` and the others are `fn` expressions
or named functions. A chain can put one call per line:

```brim
fn main() {
    let words = "the quick brown fox".split(" ")
    let long = words
        .filter(fn(w: string) -> bool { return w.len() > 3 })
        .map(fn(w: string) -> string { return w.to_upper() })
    print(long)
    print(words.fold(0, fn(n: i64, w: string) -> i64 { return n + w.len() }))
}
```

```text
["QUICK", "BROWN"]
16
```

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

## Sets

`Set[T]` holds distinct values, in the order they were first added.
`Set()` makes an empty one and `Set(list)` one from a list.

| Method | Result | |
|--------|--------|---|
| `add(x)`, `remove(x)` | , `bool` | `remove` tells whether `x` was there |
| `contains(x)` | `bool` | |
| `len()`, `is_empty()`, `to_list()` | | |
| `union(s)`, `intersection(s)`, `difference(s)` | `Set[T]` | |
| `is_subset(s)` | `bool` | |

```brim
fn main() {
    let mut tags: Set[string] = Set()
    tags.add("rust")
    tags.add("brim")
    tags.add("rust")
    print(tags)
    let other = Set(["brim", "go"])
    print(tags.intersection(other))
    for tag in tags.union(other) {
        print(tag)
    }
}
```

```text
{"rust", "brim"}
{"brim"}
rust
brim
go
```

## Option and Result

`Option[T]` is a value that may be absent (`Some(x)` or `None`);
`Result[T, E]` is the outcome of something that can fail (`Ok(x)` or
`Err(e)`). Inspect them with `match`, or pass a failure on with `?`.
Their methods: `is_some()`, `is_none()`, `unwrap_or(fallback)` and
`map(f)` on an `Option`; `is_ok()`, `is_err()`, `unwrap_or(fallback)`,
`ok()` and `map(f)` on a `Result`.

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

## Networking: std.net

`use @std/net` gives TCP connections. Everything that can fail returns a
`Result` whose error says what went wrong, such as
`cannot connect to example.com:81: connection refused`.

| Function or method | Result | |
|--------------------|--------|---|
| `net.listen(host, port)` | `Result[Listener, string]` | port 0 picks a free port |
| `net.connect(host, port)` | `Result[Conn, string]` | |
| `listener.accept()` | `Result[Conn, string]` | waits for a connection |
| `listener.next_ready(idle_millis)` | `Result[Conn, string]` | a new connection, or one given back with `listener.keep(conn)` that has something to read |
| `listener.port()` | `i64` | |
| `conn.read_line()` | `Result[string, string]` | without the line ending |
| `conn.read_until(delimiter)` | `Result[string, string]` | the delimiter is dropped |
| `conn.read_exact(count)` | `Result[string, string]` | `count` bytes |
| `conn.read_all()` | `Result[string, string]` | until the other side closes |
| `conn.write(text)` | `Result[i64, string]` | the number of bytes |
| `conn.set_timeout(millis)` | `Result[i64, string]` | 0 waits forever |
| `close()` | | on a listener or a connection |

```brim
use @std/net

fn main() {
    match greet() {
        Ok(_) => print("done")
        Err(e) => print("error: " + e)
    }
}

// Answers one connection on port 7000 with a greeting.
fn greet() -> Result[i64, string] {
    let listener: net.Listener = net.listen("127.0.0.1", 7000)?
    let conn: net.Conn = listener.accept()?
    let name: string = conn.read_line()?
    conn.write("Hello, " + name + "!\n")?
    conn.close()
    return Ok(0)
}
```

## HTTP: std.http

`use @std/http` gives an HTTP/1.1 server and client for text, such as
HTML and JSON. The server handles one request at a time: ask for the next
request, decide what to answer, and respond. Connections stay open for the
client's next request (keep-alive), and a quiet one keeps no other client
waiting.

```brim
use @std/http

fn main() {
    match serve() {
        Ok(_) => print("stopped")
        Err(e) => print("error: " + e)
    }
}

fn serve() -> Result[i64, string] {
    let server: http.Server = http.serve("0.0.0.0", 8080)?
    while true {
        let request: http.Request = server.next()?
        request.respond(route(request))?
    }
}

fn route(request: http.Request) -> http.Response {
    if request.method == "GET" && request.path == "/hello" {
        match request.param("name") {
            Some(name) => return http.text(200, "Hello, " + name + "!")
            None => return http.text(200, "Hello!")
        }
    }
    if request.method == "POST" && request.path == "/echo" {
        return http.json(200, request.body)
    }
    return http.not_found()
}
```

| Server | |
|--------|---|
| `http.serve(host, port)` | `Result[Server, string]` |
| `server.next()` | the next `Request`; malformed requests get a 400 automatically |
| `request.method`, `path`, `query`, `headers`, `body` | the request; header names in lower case, query parameters decoded |
| `request.param(name)`, `request.header(name)` | `Option[string]` |
| `request.respond(response)` | sends the response and closes the connection |
| `http.text(status, body)`, `http.json(...)`, `http.html(...)` | responses with a content type |
| `http.response(status, content_type, body)`, `http.redirect(url)`, `http.not_found()` | |
| `response.with_header(name, value)` | the response with one more header |

| Client | |
|--------|---|
| `http.get(url)` | `Result[Response, string]` |
| `http.post(url, content_type, body)` | |
| `http.request(method, url, headers, body)` | any method, with extra headers |
| `response.status`, `headers`, `body` | |
| `response.ok()` | whether the status is 2xx |
| `response.header(name)` | `Option[string]` |

`serve_api(module, request)` answers requests for the `pub fn`s of a
module (`POST /api/<name>`, parameters as a JSON object); see
[Tools](tools.md) for the generated Angular client.
`http.new_request(method, path, body)` makes a request for testing a
handler.

The client supports `http://` and `https://` URLs. HTTPS goes through
libcurl and checks certificates; redirects are not followed. `http.encode`
and `http.decode` percent-encode and decode parts of URLs.

## JSON: std.json

`use @std/json` reads and writes JSON. A value is a `json.Json`:
`Null`, `Bool`, `Number` (kept as the text it was written with), `Str`,
`Array` or `Object`.

```brim
use @std/json

fn main() {
    match age_next_year("{\"name\": \"Ada\", \"age\": 36}") {
        Ok(n) => print(n)
        Err(e) => print("error: " + e)
    }
    let reply: json.Json = json.object([
        "ok": json.from_bool(true),
        "items": json.array([json.from_int(1), json.from_string("two")]),
    ])
    print(json.stringify(reply))
}

fn age_next_year(text: string) -> Result[i64, string] {
    let user: json.Json = json.parse(text)?
    match user.get("age") {
        Some(age) => {
            match age.as_int() {
                Some(n) => return Ok(n + 1)
                None => return Err("age is not a whole number")
            }
        }
        None => return Err("no age")
    }
}
```

| | |
|---|---|
| `json.parse(text)` | `Result[Json, string]`; errors say where: `expected ':' at character 6` |
| `json.stringify(value)`, `json.pretty(value)` | compact, or indented by two spaces |
| `json.null()`, `from_bool(b)`, `from_int(n)`, `from_float(x)`, `from_string(s)`, `array(items)`, `object(members)` | build values |
| `value.get(key)`, `value.at(index)` | `Option[Json]`: a member, an element |
| `value.as_string()`, `as_int()`, `as_float()`, `as_bool()`, `as_array()`, `as_object()` | `Option` of the contents |
| `value.is_null()`, `value.kind()` | `"null"`, `"bool"`, `"number"`, `"string"`, `"array"` or `"object"` |

### Typed values

`json.encode(value)` writes any value as JSON, and `json.decode(text)`
reads it back into the type that is expected. Structs become objects,
`Option` is the value or `null`, enums are `"Red"` or
`{"kind": "Circle", "value": 1.5}`, and lists, sets and tuples are arrays:
the same shapes `brim gen ts` declares for TypeScript.

```brim
use @std/json

struct Note {
    id: i64
    text: string
    tags: List[string]
}

fn main() {
    let text = json.encode(Note { id: 1, text: "milk", tags: ["shop"] })
    print(text)
    match read("{\"id\": 2, \"text\": \"tea\", \"tags\": [7]}") {
        Ok(note) => print(note)
        Err(e) => print(e)
    }
}

fn read(text: string) -> Result[Note, string] {
    let note: Note = json.decode(text)?
    return Ok(note)
}
```

```text
{"id":1,"text":"milk","tags":["shop"]}
at $.tags[0]: expected a string, found a number
```

Every type without functions in it implements the built-in trait `Data`;
generic code that encodes or decodes a `T` needs `T: Data`.
`json.to_json(value)` and `json.from_json(json)` do the same with a
`json.Json` instead of text.

`examples/http/notes.brim` combines `std.http` and `std.json` into a small
JSON API.

## Databases: std.postgres

`std.postgres` talks to a PostgreSQL server. Parameters go in the SQL as
`$1`, `$2`, ... and travel separately, so values never become SQL text.
`query_as` reads each row into a struct whose fields are the columns.

```brim
use @std/postgres

struct Note {
    id: i64
    text: string
}

fn notes(url: string) -> Result[List[Note], string] {
    let db = postgres.connect(url)?
    db.execute("insert into notes (text) values ($1)", [postgres.text("milk")])?
    let found: List[Note] = db.query_as("select id, text from notes where id > $1", [postgres.int(0)])?
    db.close()
    return Ok(found)
}
```

| | |
|---|---|
| `postgres.connect(url)` | `postgres://user:password@host:port/database` |
| `db.query(sql, params)` | `Result[Rows, string]`: `columns`, `rows`, `affected` |
| `db.execute(sql, params)` | the number of rows changed |
| `db.query_as(sql, params)` | `Result[List[T], string]` for any `T: Data` |
| `db.transaction(work)` | runs `work(db)`; commits on `Ok`, rolls back on `Err` |
| `row.get(column)` | `Option[string]`; `None` for NULL |
| `row.text(column)`, `int`, `float`, `bool` | `Result`s |
| `postgres.null()`, `int(n)`, `float(x)`, `text(s)`, `bool(b)`, `json_value(v)` | parameters |

Logins use SCRAM-SHA-256, MD5 or a plain password. The connection is not
encrypted: connect over localhost, a private network or a tunnel. Errors
read like `postgres: ERROR: relation "x" does not exist [42P01]`.

## Hashes and encodings: std.crypto, std.base64

| | |
|---|---|
| `crypto.sha256(bytes)`, `crypto.sha256_hex(text)` | SHA-256 |
| `crypto.hmac_sha256(key, message)` | HMAC-SHA-256 |
| `crypto.pbkdf2_sha256(password, salt, iterations, length)` | a key from a password, also for storing passwords |
| `crypto.md5(bytes)` | only where another system needs it |
| `crypto.random_bytes(n)` | secure random bytes, for keys and tokens |
| `crypto.hex(bytes)`, `crypto.equal(a, b)` | hex digits; a comparison that does not leak where secrets differ |
| `base64.encode(bytes)`, `base64.decode(text)` | RFC 4648; `encode_text` and `decode_text` for strings |

Bytes are `List[u8]`: `text.bytes()` and `string_from_bytes(bytes)` convert.

```brim
use @std/base64
use @std/crypto

fn main() {
    print(crypto.sha256_hex("abc"))
    print(base64.encode_text("hello"))
}
```

```text
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
aGVsbG8=
```

## Random numbers: std.random

```brim
use @std/random

fn main() {
    random.seed(42)
    let die = random.int(1, 7)
    print(die >= 1 && die <= 6)
    print(random.shuffle([1, 2, 3]).sorted())
    print(random.choice(["a", "b"]).is_some())
}
```

```text
true
[1, 2, 3]
true
```

`int(low, high)` is like the range `low..high`: `high` itself never
comes up. `float()` is from 0 up to 1; `bool()`, `choice(list)` and
`shuffle(list)` do what they say. Without `seed(n)` the numbers differ on
every run; with it they are the same, which tests need. The generator is
not meant for passwords or keys.

## Running programs: std.process

`process.run(cmd, args)` starts a program, waits until it has finished and
gives a `process.Output`: the exit code and everything it wrote to
standard output and standard error, as strings. The program is looked up in
`PATH` unless its name contains a `/`; the arguments reach it as they are,
without a shell in between. It gets no input.

| Function | Result | |
|----------|--------|---|
| `run(cmd, args)` | `Result[Output, string]` | `Err` only if the program cannot start or writes text that is not UTF-8 |
| `output(cmd, args)` | `Result[string, string]` | the standard output if the exit code is 0; otherwise an error with the standard error |
| `out.success()` | `bool` | whether `out.code` is 0 |

`out.code` is the exit code, or 128 plus the number of the signal that
stopped the program (137 for `kill -9`).

```brim
use @std/process

fn main() {
    match process.run("sh", ["-c", "echo hello; exit 3"]) {
        Ok(out) => print(f"{out.code} {out.stdout.trim()}")
        Err(message) => print(message)
    }
    match process.output("no-such-program", []) {
        Ok(text) => print(text)
        Err(message) => print(message)
    }
}
```

```text
3 hello
cannot run "no-such-program": not found
```

## Time and logging: std.time, std.log

```brim
use @std/log
use @std/time

fn main() {
    let start: i64 = time.millis()
    print(time.now().iso())
    print(time.from_unix_seconds(0).http_date())
    time.sleep(10)
    log.info("took " + to_string(time.millis() - start) + " ms")
}
```

| | |
|---|---|
| `time.now()` | the current `Time`, from the system clock, in UTC |
| `time.from_unix_millis(ms)`, `time.from_unix_seconds(s)` | a `Time` |
| `t.iso()`, `t.date()`, `t.http_date()` | `2026-10-15T08:30:01.123Z`, `2026-10-15`, `Thu, 15 Oct 2026 08:30:01 GMT` |
| `t.parts()` | `year`, `month`, `day`, `hour`, `minute`, `second`, `millisecond`, `weekday` (1 is Monday) |
| `t.unix_seconds()`, `t.add_millis(n)`, `t.add_seconds(n)` | |
| `time.millis()` | milliseconds on a clock that only moves forward, for durations |
| `time.sleep(ms)` | waits |
| `log.info(m)`, `log.warn(m)`, `log.error(m)` | `2026-10-15T08:30:01.123Z INFO m` on standard error |

## Tasks: std.task

Tasks run functions at the same time, each on a thread of its own. They
share no changeable memory: a `fn` copies what it uses, and a value a task
changes is its own copy. Tasks work together through `join` and channels.

```brim
use @std/task

fn square_all(numbers: List[i64]) -> i64 {
    let jobs: task.Channel[i64] = task.channel()
    let results: task.Channel[i64] = task.channel()
    // Four workers take jobs until the channel is closed.
    for _ in 0..4 {
        task.detach(fn() {
            loop {
                match jobs.receive() {
                    Some(n) => results.send(n * n)
                    None => break
                }
            }
        })
    }
    for n in numbers {
        jobs.send(n)
    }
    jobs.close()
    let mut total = 0
    for _ in numbers {
        total += results.receive().unwrap_or(0)
    }
    return total
}

fn main() {
    let sum = task.spawn(fn() -> i64 { return square_all([1, 2, 3, 4]) })
    print(sum.join())
}
```

```text
30
```

| | |
|---|---|
| `task.spawn(f)` | runs `f`, a `fn() -> T`, on a new task: a `Task[T]` |
| `t.join()` | waits for the task and returns its result (once) |
| `task.detach(f)` | runs `f`, a `fn()`, on a task that is never joined |
| `task.channel()` | a `Channel[T]` without a size limit |
| `task.bounded(n)` | a `Channel[T]` whose `send` waits while `n` values are queued |
| `c.send(value)` | queues a value; `false` once the channel is closed |
| `c.receive()` | waits for the next value: `Some(value)`, or `None` once the channel is closed and empty |
| `c.close()` | no more values |

A service handles every request on a task of its own:

```brim
use @std/http
use @std/log
use @std/task

fn serve(port: i64) -> Result[bool, string] {
    let server = http.serve("127.0.0.1", port)?
    loop {
        let request = server.next()?
        task.detach(fn() {
            let response = http.text(200, f"Hello from {request.path}")
            match request.respond(response) {
                Ok(_) => {}
                Err(e) => log.warn(e)
            }
        })
    }
}

fn main() {
    match serve(8080) {
        Ok(_) => {}
        Err(e) => log.error(e)
    }
}
```

A run-time error in a task stops the whole program, and the program ends
when `main` returns, also while detached tasks still run.

## Tests

| Function | |
|----------|---|
| `assert(condition)` | stops the program when the condition is false |
| `assert_eq(a, b)` | stops the program when the values differ, and shows both; for `i64`, `bool` and `string` |

`brim test` runs every function whose name starts with `test_`; see
[Projects, tests and dependencies](projects.md).
