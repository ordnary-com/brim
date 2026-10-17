# Brim Language Specification

**Version:** 1.0, edition `2026`
**Status:** describes exactly what the reference compiler in this repository accepts. Anything not described here is not part of the language. The design decisions behind it are in [section 11](#11-design-decisions), and what 1.0 leaves out in [section 12](#12-not-in-10).

This document is normative for the language. The implementation lives in `crates/`, and where the two disagree, that is a bug in one of them.

## 1. Overview

Brim is a statically typed, compiled, general-purpose language from Ordnary, aimed at backend services, APIs, CLI tools and developer tooling. Its values are values: structs, enums, lists, maps and sets are copied on assignment (cheaply, with copy-on-write), there is no null (`Option`) and there are no exceptions (`Result` and `?`), `match` must cover every case, and integer overflow is an error rather than a wrap. Programs run in an interpreter and compile to native code through LLVM, with the same behaviour. Generics, traits, closures, modules, packages and tasks (section 9.7) are part of the language; section 10 lists the builtins and the standard library.

```brim
fn fizzbuzz(n: i64) -> string {
    if n % 15 == 0 {
        return "FizzBuzz"
    } else if n % 3 == 0 {
        return "Fizz"
    } else if n % 5 == 0 {
        return "Buzz"
    }
    return to_string(n)
}

fn main() {
    let mut i: i64 = 1
    while i <= 15 {
        print(fizzbuzz(i))
        i += 1
    }
}
```

## 2. Source text

- A source file is UTF-8 text with the extension `.brim`. A leading byte order mark (U+FEFF) is ignored. Files that are not valid UTF-8 are rejected.
- Line terminators are `\n` or `\r\n`. A lone `\r` is treated as whitespace.
- Positions in diagnostics are reported as `line:column`, both 1-based. Columns count Unicode scalar values, not bytes. Internally the compiler stores byte offsets (see `docs/architecture.md`).

## 3. Lexical structure

### 3.1 Whitespace and comments

Spaces, tabs and carriage returns separate tokens and are otherwise ignored. **Newlines are significant**: they terminate statements (section 6.1).

A line comment starts with `//` and extends to the end of the line. The newline that ends a comment is still a newline token. There are no block comments.

### 3.2 Identifiers

```ebnf
identifier = ( letter | "_" ) { letter | digit | "_" } ;
letter     = "A".."Z" | "a".."z" ;
digit      = "0".."9" ;
```

Identifiers are ASCII-only and case-sensitive. A word that is a keyword or a reserved word is never an identifier.

### 3.3 Keywords

Keywords:

```
fn  let  mut  return  if  else  while  break  continue  true  false  struct
enum  match  for  in  impl  trait  const  loop  self  import  pub  as  extern
```

Reserved words for planned features. They cannot be used as identifiers, and using one produces error `E0101`:

```
async  await  type  use
```

The type names `i64`, `bool`, `string` and `void` are **not** keywords. They are predeclared type names, resolved during semantic analysis.

### 3.4 Integer literals

```ebnf
integer_literal = digit { digit | "_" } ;
```

- Decimal only. `_` may be used as a separator (`1_000_000`) and is ignored.
- A literal with more than one digit may not start with `0` (`007` is error `E0005`), to avoid confusion with octal notation in other languages.
- Letters directly after a literal (`10px`, `0xFF`) are error `E0006`. Hexadecimal, octal and binary literals are not supported yet.
- A literal has the integer type the context expects (`let b: u8 = 200`, `f(3)` with an `i32` parameter, `x + 1` with `x: u64`), and `i64` otherwise. Its value must fit that type: at most 9223372036854775807 for `i64`, 18446744073709551615 for `u64`. A literal directly after unary `-` is one negative value, so `-9223372036854775808` is `i64::MIN` and `-1` is not a `u64`. Out-of-range literals are error `E0215`. Literals that do not even fit in 64 bits are error `E0004`.

### 3.4.1 Floating-point literals

```ebnf
float_literal = digit { digit | "_" } ( "." digit { digit | "_" } [ exponent ] | exponent ) ;
exponent      = ( "e" | "E" ) [ "+" | "-" ] digit { digit | "_" } ;
```

- `1.5`, `0.25`, `6.02e23`, `1.5E-7` and `2e10` are `f64` literals; `_` separators are allowed as in integers.
- There must be digits on both sides of the point: `1.` and `.5` are not literals. `1..5` is a range, and `point.0` a field.
- A literal too large for `f64` (`1e999`) is error `E0007`. The value is the `f64` nearest to the decimal number.

### 3.5 String literals

```ebnf
string_literal = '"' { string_char | escape } '"' ;
string_char    = ? any Unicode scalar value except '"', '\' and newline ? ;
escape         = '\' ( 'n' | 'r' | 't' | '0' | '\' | '"' )
               | '\u{' hex_digit { hex_digit } '}' ;   (* 1 to 6 hex digits *)
```

- A string literal must end on the line where it starts (`E0002`). Use `\n` for line breaks.
- `\u{...}` must denote a Unicode scalar value: not a surrogate and at most `10FFFF`.
- Any other escape is error `E0003`.

#### 3.5.1 f-strings

```ebnf
fstring = 'f"' { string_char | escape | '{{' | '}}' | '{' expression '}' } '"' ;
```

An **f-string** puts values in text: `f"Hello, {name}! Next year you are {age + 1}."`. Each `{expression}` is shown as `print` shows it: strings as they are, numbers and booleans as `to_string` writes them, and structs, enums, lists and maps as in section 5.8. A value that cannot be shown (a function, or a call that returns nothing) is `E0206`. `{{` and `}}` are a `{` and `}` in the text, and a lone `}` is `E0001`. The expressions may contain strings and other f-strings, and the whole f-string must end on its line. A string without the `f` keeps every `{` as it is, so JSON in a string needs no escaping.

### 3.6 Operators and punctuation

```
+  -  *  /  %  ==  !=  <  <=  >  >=  &&  ||  !
=  +=  -=  *=  /=  %=
(  )  {  }  [  ]  ,  :  ;  ->  .  ..  =>  ?
```

Any other character outside a string or comment is error `E0001`. Single `&` and `|` are errors too, with a hint to use `&&` and `||`.

## 4. Grammar

The grammar below uses EBNF. `NEWLINE` is a newline token; `EOF` is the end of input.

```ebnf
program     = { separator } { ( import | item ) { separator } } EOF ;
import      = [ "pub" ] "import" path [ "." "{" identifier { "," identifier } [ "," ] "}" | "as" identifier ] ;
item        = [ "pub" ] ( function | extern_fn | struct | enum | trait | const ) | impl ;
extern_fn   = "extern" "fn" identifier "(" [ param { "," param } [ "," ] ] ")" [ "->" type ] ;
impl        = "impl" [ path "for" ] identifier [ type_params ] "{" { separator } { [ "pub" ] function { separator } } "}" ;
trait       = "trait" identifier "{" { separator } { trait_fn { separator } } "}" ;
trait_fn    = "fn" identifier "(" "self" { "," param } [ "," ] ")" [ "->" type ] ;
const       = "const" identifier [ ":" type ] "=" expression ;
separator   = NEWLINE | ";" ;
path        = identifier { "." identifier } ;

struct      = "struct" identifier [ type_params ] "{" { field_sep } [ field { field_sep { field_sep } field } ] { field_sep } "}" ;
field       = [ "pub" ] identifier ":" type ;
field_sep   = NEWLINE | ";" | "," ;

enum        = "enum" identifier [ type_params ] "{" { field_sep } [ variant { field_sep { field_sep } variant } ] { field_sep } "}" ;
variant     = identifier [ "(" type { "," type } [ "," ] ")" ] ;

type_params = "[" type_param { "," type_param } [ "," ] "]" ;
type_param  = identifier [ ":" path { "+" path } ] ;

function    = "fn" identifier [ type_params ] "(" [ params ] ")" [ "->" type ] block ;
params      = ( receiver [ "," param { "," param } ] | param { "," param } ) [ "," ] ;
receiver    = [ "mut" ] "self" ;
param       = identifier ":" type ;
type        = path [ "[" type { "," type } [ "," ] "]" ]
            | "(" type "," type { "," type } [ "," ] ")"                 (* a tuple *)
            | "fn" "(" [ type { "," type } [ "," ] ] ")" [ "->" type ] ;  (* a function *)

block       = "{" { separator } [ statement { terminator statement } ] { separator } "}" ;
terminator  = separator { separator } ;     (* see 6.1; "}" also ends a statement *)

statement   = let_stmt | assign_stmt | if_stmt | while_stmt | loop_stmt | for_stmt | match_stmt
            | "break" | "continue" | return_stmt | block | expr_stmt ;
let_stmt    = "let" [ "mut" ] ( identifier [ ":" type ] | tuple_pat ) "=" expression ;
assign_stmt = place assign_op expression ;
place       = identifier { "." ( identifier | integer_literal ) | "[" expression "]" } ;
assign_op   = "=" | "+=" | "-=" | "*=" | "/=" | "%=" ;
if_stmt     = "if" expression block [ "else" ( if_stmt | block ) ] ;
while_stmt  = "while" expression block ;
loop_stmt   = "loop" block ;
for_stmt    = "for" identifier "in" expression [ ".." expression ] block ;
match_stmt  = "match" expression "{" { arm_sep } [ arm { arm_sep { arm_sep } arm } ] { arm_sep } "}" ;
arm         = pattern [ "if" expression ] "=>" ( block | statement ) ;
arm_sep     = NEWLINE | ";" | "," ;
pattern     = single_pat { "|" single_pat } ;
single_pat  = "_" | [ "-" ] integer_literal | string_literal | "true" | "false" | tuple_pat
            | [ identifier "." ] identifier [ "(" pattern { "," pattern } [ "," ] ")" ] ;
tuple_pat   = "(" pattern "," pattern { "," pattern } [ "," ] ")" ;
return_stmt = "return" [ expression ] ;
expr_stmt   = expression ;

expression  = or_expr ;
or_expr     = and_expr { "||" and_expr } ;
and_expr    = eq_expr { "&&" eq_expr } ;
eq_expr     = cmp_expr [ ( "==" | "!=" ) cmp_expr ] ;
cmp_expr    = add_expr [ ( "<" | "<=" | ">" | ">=" ) add_expr ] ;
add_expr    = mul_expr { ( "+" | "-" ) mul_expr } ;
mul_expr    = unary { ( "*" | "/" | "%" ) unary } ;
unary       = ( "-" | "!" ) unary | postfix ;
postfix     = primary { call | "." ( identifier | integer_literal ) | struct_lit | "[" expression "]" | "?" } ;
call        = "(" [ args ] ")" ;
struct_lit  = "{" [ field_init { "," field_init } [ "," ] ] "}" ;   (* only directly after a type's name *)
field_init  = identifier ":" expression ;
args        = expression { "," expression } [ "," ] ;
primary     = integer_literal | float_literal | string_literal | fstring | "true" | "false" | "self"
            | identifier | "(" expression ")" | tuple_lit | list_lit | map_lit
            | if_stmt | match_stmt | fn_expr ;
tuple_lit   = "(" expression "," expression { "," expression } [ "," ] ")" ;
list_lit    = "[" [ expression { "," expression } [ "," ] ] "]" ;
map_lit     = "[" ":" "]" | "[" expression ":" expression { "," expression ":" expression } [ "," ] "]" ;
fn_expr     = "fn" "(" [ param { "," param } [ "," ] ] ")" [ "->" type ] block ;
```

### 4.1 Precedence and associativity

From loosest to tightest binding:

| Level | Operators              | Associativity      |
|-------|------------------------|--------------------|
| 1     | `\|\|`                 | left               |
| 2     | `&&`                   | left               |
| 3     | `==` `!=`              | none (no chaining) |
| 4     | `<` `<=` `>` `>=`      | none (no chaining) |
| 5     | `+` `-`                | left               |
| 6     | `*` `/` `%`            | left               |
| 7     | unary `-` `!`          | prefix             |
| 8     | call `f(...)`, field `.x`, index `[i]`, struct literal `S { ... }`, `?` | postfix |

Comparison operators at the same level cannot be chained: `a < b < c` and `a == b == c` are error `E0106`. Comparisons at different levels compose: `a < b == c > d` means `(a < b) == (c > d)`.

### 4.2 Restrictions enforced by the parser

- Only imports and items may appear at the top level; a `let` there is `E0103` (a constant is a `const`). Items other than functions (`struct`, `enum`, `trait`, `impl`, `const`) cannot be declared inside functions (`E0103`).
- In the condition of an `if` or `while`, `Name {` starts the body, not a struct literal: write `if p == (Point { x: 0, y: 0 }) {`, with parentheses, to use a literal there.
- A line that starts with `.` continues the expression on the line above it (section 6.1).
- `let` requires an initial value (`E0100`); parameters and fields require a type (`E0102`).
- Only a variable or a field path starting at a variable (`p.from.x`) can be assigned to: `f() = 1` is error `E0104`.
- The body of `if`, `else` and `while` must be a block in braces. `else` must be followed by `{` or `if`.
- A call's callee is a function name, a variable or field holding a function value, or any expression of a function type: `f(1)`, `button.on_click(3)`, `make_adder(1)(2)`. Calling a value that is not a function is `E0107` (or `E0214` for a variable).
- Blocks and expressions may nest at most 128 levels deep, counting long operator chains and `else if` chains (`E0105`). This is an implementation limit that protects the compiler's stack.

## 5. Types

| Type     | Values                                    |
|----------|-------------------------------------------|
| `i64`    | 64-bit signed integers, two's complement  |
| `i32`    | 32-bit signed integers                    |
| `u8`     | integers from 0 to 255, used for bytes    |
| `u64`    | integers from 0 to 2^64 - 1               |
| `f64`    | 64-bit floating-point numbers (IEEE 754 double precision) |
| `bool`   | `true`, `false`                           |
| `string` | immutable sequences of Unicode text (UTF-8) |
| `void`   | no values                                 |
| structs  | declared with `struct` (section 5.1)      |
| enums    | declared with `enum` (section 5.2); `Option[T]` and `Result[T, E]` are built in (section 5.4) |
| `List[T]` | ordered sequences of `T` (section 5.5)  |
| `Map[K, V]` | values of type `V` looked up by keys of type `K` (section 5.6) |
| `Set[T]` | distinct values of type `T` (section 5.6.1) |
| `Box[T]` | one value of type `T` on the heap, so a type can contain itself (section 5.7) |
| `(A, B, ...)` | tuples: two or more values of the given types (section 5.13) |

- `void` may only appear as a function return type. Variables and parameters of type `void` are error `E0213`.
- Unknown type names are error `E0212`.
- There are **no implicit conversions** between types, ever: `to_float(n)` turns an integer into an `f64` and `to_int(x)` the other way (section 10).
- **Integer types.** Arithmetic and comparisons work on two integers of the same type, and the result has that type. An operation whose result does not fit the type stops the program with run-time error `R0001` (`u8` 250 + 10, `i32` 2147483647 + 1, `u64` 0 - 1); `u8` and `u64` cannot be negated. `to_int(x)`, `to_i32(x)`, `to_u8(x)` and `to_u64(x)` convert between integer types, with `R0001` when the value does not fit. `i32` and `u8` values currently take as much memory as an `i64`.
- **Bytes** are a `List[u8]`: `s.bytes()` gives a string's UTF-8 bytes and `string_from_bytes(b)` the text back, or `None` when the bytes are not valid UTF-8.
- Two types are compatible only if they are identical.

### 5.1 Structs

```brim
struct Point {
    x: i64
    y: i64
}
```

A struct declaration introduces a new type with named, typed **fields**. Fields are separated by newlines or commas. Struct names share the type namespace with the built-in types (`struct string {}` is `E0219`) and may not be used by a function as well (`E0219`). A struct can be used before its declaration.

- Field names within a struct must be unique (`E0220`). A field cannot have type `void` (`E0213`).
- A struct cannot contain itself, directly or through other structs or enums (`E0221`): structs are values, so that would make it infinitely large. It can contain itself through a `List`, `Map` or `Box`, whose contents live on the heap: `struct Node { name: string, children: List[Node] }` is a tree.

**Values.** `Point { x: 1, y: 2 }` creates a struct value. Every field must be given exactly once (`E0222` for a missing field, `E0223` for a repeated one, `E0224` for an unknown one), in any order. The field values are evaluated in the order they are written. `p.x` reads a field; reading a field of a non-struct is `E0224`.

**Value semantics.** Structs are values, like integers: assignment, passing an argument and returning copy the whole struct, and changing a copy never changes the original. There are no references to structs.

**Field assignment.** `p.x = 3`, `p.from.y += 1` change one field of a variable. The variable must be declared with `let mut` (`E0218`); its fields are as mutable as the variable itself.

**Visibility.** A field is visible outside the struct's module only when declared `pub` (section 9.2).

**Operations.** Two struct types are compatible only if they are the same declaration. Structs can be compared with `==` and `!=` and shown with `print` and `to_string` (section 5.8). Methods are declared in `impl` blocks.

### 5.2 Enums

```brim
enum Shape {
    Circle(i64)
    Rect(i64, i64)
    Empty
}
```

An enum declaration introduces a type whose values are exactly one of its **variants**. A variant may carry values of the listed types (its *payload*). Variants are separated by newlines or commas, and their names must be unique (`E0220`).

- `Shape.Circle(3)` and `Shape.Empty` create values. The number of values must match the variant (`E0228`), and an unknown variant is `E0227`.
- An enum is a value like a struct, and cannot contain itself directly (`E0221`), but it can through a `List`, `Map` or `Box`: `enum Json { Null, Number(i64), Array(List[Json]), Object(Map[string, Json]) }`, or `enum Expr { Num(i64), Add(Box[Expr], Box[Expr]) }` (section 5.7).
- Enums can be compared with `==` and `!=` and shown with `print` and `to_string` (section 5.8).

### 5.3 Generics

Functions, structs and enums can take **type parameters**:

```brim
struct Pair[A, B] {
    first: A
    second: B
}

fn swap[A, B](p: Pair[A, B]) -> Pair[B, A] {
    return Pair { first: p.second, second: p.first }
}
```

- A generic type is used with type arguments: `Pair[i64, string]`. The number of arguments must match (`E0225`).
- Type arguments of calls, struct literals and variants are **inferred**, from the arguments and from the type the context expects (a `let` annotation, a parameter type, a return type). Arguments that need the expected type themselves, such as `None`, are inferred last. When a type argument cannot be inferred, it is `E0226`; add an annotation.
- Inside a generic function, a type parameter can be any type, so values of that type can only be stored, passed and returned, unless the parameter has **bounds** (section 5.11): `fn total[T: Shape](xs: List[T])` may call `Shape`'s methods on a `T`, `T: Eq` allows `==` and `!=`, and `T: Show` allows `print` and `to_string`.
- Generic code is checked once and compiled once per combination of type arguments it is used with (monomorphization): there is no run-time cost.
- Type parameter names must be unique (`E0220`) and cannot be built-in type names (`E0219`).

### 5.4 `Option` and `Result`

Two generic enums are always available:

```brim
enum Option[T] { Some(T), None }
enum Result[T, E] { Ok(T), Err(E) }
```

`Option[T]` is Brim's replacement for null: a value that may be absent. `Result[T, E]` is the result of an operation that can fail. Their variants can be used without the enum's name: `Some(1)`, `None`, `Ok(x)`, `Err(message)`. The names `Option`, `Result`, `Some`, `None`, `Ok` and `Err` cannot be redefined.

**The `?` operator.** `value?` on a `Result[T, E]` evaluates to the `T` inside an `Ok`. On an `Err`, the enclosing function returns that `Err` immediately, so the function must return a `Result[U, F]` (`E0233`). When `F` is `E`, the error is returned unchanged. Otherwise `F` must be an enum with exactly one variant whose only value is an `E`, and `?` wraps the error in that variant: with `enum ConfigError { Parse(ParseError), Missing(string) }`, a `ParseError` returned through `?` becomes `Err(ConfigError.Parse(error))`. Two such variants make the conversion ambiguous, which is also `E0233`. On an `Option[T]`, `value?` evaluates to the value inside `Some`, and `None` makes the function return `None`; the function must return an `Option[U]`. `?` on any other type is `E0232`. A call followed by `?` may be used as a statement, to only propagate its error.

```brim
fn total(a: string, b: string) -> Result[i64, string] {
    return Ok(parse(a)? + parse(b)?)
}
```

**Methods** (from the prelude):

| On | Method | Result | |
|----|--------|--------|---|
| `Option[T]` | `is_some()`, `is_none()` | `bool` | |
| `Option[T]` | `unwrap_or(fallback)` | `T` | the value, or `fallback` |
| `Option[T]` | `map(f)` | `Option[U]` | `Some(f(x))`, or `None` |
| `Result[T, E]` | `is_ok()`, `is_err()` | `bool` | |
| `Result[T, E]` | `unwrap_or(fallback)` | `T` | the `Ok` value, or `fallback` |
| `Result[T, E]` | `ok()` | `Option[T]` | the `Ok` value; the error is dropped |
| `Result[T, E]` | `map(f)` | `Result[U, E]` | `Ok(f(x))`, or the same `Err` |

### 5.5 Lists

`List[T]` holds any number of values of type `T`, in order. `[1, 2, 3]` creates one; `[]` is an empty list and needs a known element type (`let xs: List[i64] = []`, `E0226`).

- `xs[i]` is element `i`, counting from 0. An index outside the list is run-time error `R0005`. `xs[i] = v` and `xs[i] += v` change an element. Indexing anything other than a list is `E0234`.
- Lists are **values**, like structs: assignment, passing and returning give an independent copy. Copies are cheap: the elements are only copied when one of the copies is changed while it is shared (copy-on-write).
- Methods:

  | Method | Result | |
  |--------|--------|---|
  | `len()` | `i64` | the number of elements |
  | `is_empty()` | `bool` | |
  | `get(i)` | `Option[T]` | the element, or `None` outside the list |
  | `first()`, `last()` | `Option[T]` | |
  | `contains(x)` | `bool` | only for lists of `i64`, `bool` and `string` |
  | `join(sep)` | `string` | only for `List[string]` |
  | `push(x)` | | appends |
  | `pop()` | `Option[T]` | removes and returns the last element |
  | `insert(i, x)` | | inserts before index `i` (`0 <= i <= len`) |
  | `remove(i)` | `T` | removes and returns element `i` |
  | `clear()` | | |
  | `sorted()` | `List[T]` | a sorted copy, smallest first; stable; `T` must implement `Ord` (section 5.11) |
  | `sorted_by(less)` | `List[T]` | a copy sorted by `less: fn(T, T) -> bool`; stable |
  | `reversed()` | `List[T]` | a copy in the opposite order |
  | `slice(start, end)` | `List[T]` | the elements from `start` up to `end`; indices outside the list are moved to its ends, so `slice` never fails |
  | `index_of(x)` | `Option[i64]` | the index of the first element equal to `x`; `T` must implement `Eq` |
  | `map(f)` | `List[U]` | `f(x)` for each element, with `f: fn(T) -> U` |
  | `filter(keep)` | `List[T]` | the elements for which `keep: fn(T) -> bool` is true |
  | `any(test)`, `all(test)` | `bool` | whether `test` holds for some, or every, element (`all` of an empty list is `true`) |
  | `fold(init, f)` | `A` | `f(...f(f(init, x0), x1)..., xn)`, with `f: fn(A, T) -> A` |
  | `find(test)` | `Option[T]` | the first element for which `test` holds |
  | `each(f)` | | calls `f: fn(T)` with each element |
  | `min()`, `max()` | `Option[T]` | the smallest or largest element; `T` must implement `Ord` |
  | `sum()` | `T` | the elements added up, 0 for an empty list; only for lists of numbers, and integer overflow stops the program |

  The methods from `push` to `clear` change the list, so they need a variable declared with `let mut` (or a field or element of one); calling them on a temporary is `E0104`. An unknown method is `E0231`.
- Lists can be compared with `==` and shown with `print` and `to_string` (section 5.8).

### 5.6 Maps

`Map[K, V]` maps keys of type `K` to values of type `V`. Keys must be integers, `bool` or `string`, or a type parameter bounded by the built-in trait `Key` (`E0236`). `["ada": 36, "alan": 41]` creates one; `[:]` is an empty map and needs known types (`E0226`).

- `m[key]` is the value for a key; a missing key is run-time error `R0006` (use `get` when the key may be missing). `m[key] = v` adds or replaces an entry; `m[key] += 1` and other changes through `m[key]` need the key to exist.
- Entries keep the **order in which their keys were first added**. Replacing a value keeps the entry's position; removing a key closes the gap.
- Maps are values with copy-on-write, like lists.
- Methods:

  | Method | Result | |
  |--------|--------|---|
  | `len()` | `i64` | the number of entries |
  | `is_empty()` | `bool` | |
  | `get(k)` | `Option[V]` | |
  | `contains_key(k)` | `bool` | |
  | `keys()` | `List[K]` | in entry order |
  | `values()` | `List[V]` | in entry order |
  | `insert(k, v)` | | adds or replaces |
  | `remove(k)` | `Option[V]` | removes the entry, returning its value |
  | `clear()` | | |

  `insert`, `remove` and `clear` change the map and need a `let mut` variable. To loop over a map, loop over `keys()` or `values()`.

#### 5.6.1 Sets

`Set[T]` holds distinct values, with the same element types as map keys. `Set()` is an empty set of the type the context expects (`E0226` without one), and `Set(list)` holds the distinct elements of a list. Like a map, a set keeps the order in which its elements were first added; `for x in set` goes through them in that order.

| Method | Result | |
|--------|--------|---|
| `add(x)` | | adds `x` if it is not there yet |
| `remove(x)` | `bool` | removes `x`; whether it was there |
| `contains(x)` | `bool` | |
| `len()`, `is_empty()` | `i64`, `bool` | |
| `to_list()` | `List[T]` | in order |
| `clear()` | | |
| `union(other)`, `intersection(other)`, `difference(other)` | `Set[T]` | new sets |
| `is_subset(other)` | `bool` | whether every element is in `other` |

`add`, `remove` and `clear` change the set and need a `let mut` variable. Two sets are equal when they have the same elements, in any order; `print` shows `{1, 2}`.


### 5.7 Boxes

`Box[T]` holds one value of type `T` on the heap. It is how a struct or enum contains itself directly:

```brim
enum Expr {
    Num(i64)
    Add(Box[Expr], Box[Expr])
}

fn eval(e: Expr) -> i64 {
    match e {
        Num(n) => return n
        Add(a, b) => return eval(a.value()) + eval(b.value())
    }
}
```

- `Box(value)` creates one; `b.value()` is the value inside. A box is a value like any other: it is immutable, and copying it shares the contents, which is safe because they cannot change.
- `==` compares the values inside, and `print` shows the value inside without the box: `Add(Num(1), Num(2))`.
- Freeing a long chain of boxes does not use the call stack, so a list built from a million links is fine.

### 5.8 Comparing and printing values

Structs, enums, lists and maps support `==` and `!=`, and `print` and `to_string` show them as text, when every value they hold does: integers, floats, booleans, strings, or other such types. A value of a type parameter does not, so inside generic code a `Box[T]` cannot be compared or printed (`E0209`, `E0206`).

- **Equality** is structural. Two structs are equal when their fields are; two enum values when they are the same variant with equal values; two lists when they have the same length and equal elements in order; two maps when they have the same keys with equal values, in any order. Floats follow IEEE 754 inside other values too, so a value holding `NaN` is not equal to itself.
- **Text** looks like the code that builds the value, with strings in quotes and escaped: `Point { x: 1, y: 2 }`, `Circle(Point { x: 1, y: 2 }, 1.5)`, `Some("ada")`, `None`, `[1, 2, 3]`, `["ada": 36]` and `[:]`. Variants are shown without their enum's name, structs without their type arguments.

```brim
let a: Point = Point { x: 1, y: 2 }
print(a == Point { x: 1, y: 2 })   // true
print([a])                         // [Point { x: 1, y: 2 }]
```

The right side of `==` and `!=` is checked with the left side's type in mind, so `o == None` needs no annotation.

### 5.9 Methods

```brim
impl Stack[T] {
    fn new() -> Stack[T] { return Stack { items: [] } }
    fn size(self) -> i64 { return self.items.len() }
    fn push(mut self, item: T) { self.items.push(item) }
}
```

An `impl` block adds **methods** to a struct or enum declared in the same file. A generic type's `impl` lists its type parameters (`impl Stack[T]`), which the methods can use; methods may also have their own.

- A method whose first parameter is `self` is called on a value: `stack.size()`. `self` is a copy of the value, like any parameter.
- With `mut self`, the method changes the caller's value: `stack.push(1)` replaces `stack` with the changed `self` when the method returns. The value must be a `let mut` variable, or a field or element of one (`E0218`, `E0104`). The value is moved into the method, so changing a list inside `self` does not copy it.
- A method without `self` is called on the type: `Stack.new()`.
- Calling a method the wrong way is `E0240`; an unknown method is `E0231`. Methods of one type need different names, and an enum's methods cannot share a name with its variants (`E0238`). Built-in types cannot get methods (`E0237`), and `self` outside an `impl` is `E0239`.
- `self` is a keyword; inside a method it is a variable like a parameter.

### 5.10 Functions as values

`fn(A, B) -> R` is the type of functions taking an `A` and a `B` and returning an `R`; `fn(A)` returns nothing. Values of such a type are made two ways:

- A **named function** used without calling it: `apply(double, 21)`. Generic functions cannot be used this way (`E0214`); wrap them in a `fn` expression. Builtins cannot either.
- A **`fn` expression**: `fn(x: i64) -> i64 { return x * 2 }`. Its parameters need types, and its body is checked like a function's (`E0207` when a non-`void` one can end without `return`).

```brim
fn adder(n: i64) -> fn(i64) -> i64 {
    return fn(x: i64) -> i64 {
        return x + n
    }
}

fn main() {
    let add10: fn(i64) -> i64 = adder(10)
    print(add10(5))          // 15
}
```

- A `fn` expression can use the variables around it. It **copies** each one when the expression is evaluated (it *captures* it), like every other value in Brim, so later changes to the variable do not reach the function, and the function cannot change the variable (`E0218`). To change something, return the new value.
- A function value is called like a function: `f(3)`, `button.on_click(3)` for a field (when the struct has no method of that name), or `make_adder(1)(2)`.
- Function values can be stored in variables, fields, lists and enums, passed and returned. They cannot be compared with `==` or printed (`E0209`, `E0206`).
- A run-time error inside a `fn` expression shows it in the call stack as `enclosing.<fn>`.

### 5.11 Traits

A **trait** names methods that several types can have:

```brim
trait Shape {
    fn area(self) -> f64
    fn scaled(self, factor: f64) -> Self
}

struct Circle {
    r: f64
}

impl Shape for Circle {
    fn area(self) -> f64 {
        return 3.14 * self.r * self.r
    }
    fn scaled(self, factor: f64) -> Circle {
        return Circle { r: self.r * factor }
    }
}

fn total_area[T: Shape](shapes: List[T]) -> f64 {
    let mut total: f64 = 0.0
    for s in shapes {
        total = total + s.area()
    }
    return total
}
```

- A trait method is a signature without a body. It takes `self` (not `mut self`) and is called on a value. `Self` in its types is the type that implements the trait (`E0246` for malformed traits, unknown traits and duplicates).
- `impl Trait for Type` implements a trait for a struct or enum declared in the same module. It must have every method of the trait, with the same signature (`Self` replaced by the type), and nothing else; other methods go in a plain `impl Type` block (`E0247`). A type implements a trait at most once. The methods are also ordinary methods of the type.
- A type parameter of a function can have **bounds**, `[T: Shape + Show]`: the type arguments must implement every bound (`E0248`), and inside the function a `T` has the bounds' methods.
- The built-in traits **`Eq`** and **`Show`** are implemented by every type whose values can all be compared with `==`, or shown with `print` (section 5.8), and by type parameters bounded by them. **`Ord`** allows `<`, `<=`, `>` and `>=` and is implemented by the integer types, `f64` and `string`. **`Key`** allows a type to be a map key or set element, and is implemented by the integer types, `bool` and `string`. **`Data`** allows a type to be turned into JSON and back (`json.encode`, `json.decode`) and, like `Eq`, is implemented by every type without function values in it. They cannot be implemented by hand.
- Methods of built-in types that are written in Brim itself, such as `sorted` on lists, live in the module `std.prelude`, which every program has.
- Generic code is compiled per type, so a call through a bound is an ordinary call of the type's method. Traits are not types: a `List` holds values of one type, and there are no trait objects.

### 5.12 Constants

`const NAME: T = value` at the top level of a module names a value; the type may be left out, as with `let`. The value is computed from literals, operators, other constants, and struct, enum, list, map and box values; calls and variables are not allowed (`E0249`, also for constants defined in terms of themselves). A constant is used by name, and a `pub const` from another module as `module.NAME`. Two constants with the same name are `E0202`. `const` inside a function is `E0103`: use `let`.

```brim
const MAX_TRIES: i64 = 5
const ORIGIN: Point = Point { x: 0, y: 0 }
```

### 5.13 Tuples

A **tuple** groups two or more values without naming them: `(i64, string)` is the type, `("ada", 36)` a value (a single value in parentheses is just that value).

```brim
fn divmod(a: i64, b: i64) -> (i64, i64) {
    return (a / b, a % b)
}

let (q, r) = divmod(17, 5)
```

- `t.0`, `t.1`, ... are the values; `t.0.1` reads into a nested tuple. Fields of a tuple in a `let mut` variable can be assigned (`t.0 = 5`). A field past the end is `E0224`.
- `(p, q)` is a pattern in `match`. `let (a, b) = value` and `let mut (a, b) = value` take a tuple apart; the pattern must match every value (`E0251`), so it can use names, `_` and nested tuples but not literals that could fail.
- Tuples are values like structs: `==`, `print` and `to_string` work when their values do (`(36, "ada")`), and they can be map values, list elements and type arguments. `brim gen ts` writes them as TypeScript tuples.

## 6. Statements

### 6.1 Statement termination

A statement ends at a newline, a `;`, or the `}` that closes its block. Two statements on one line must be separated by `;`. Empty statements, such as `;;` or blank lines, are allowed.

Newlines do **not** end a statement:

- inside parentheses, so argument and parameter lists may span lines;
- directly after a binary operator, an assignment operator (`=`, `+=`, ...), `->` or `,`;
- before the `{` that opens a function body or the block of an `if`, `else` or `while`;
- between the `}` of an `if` block and a following `else`;
- before a line that starts with `.`, which continues the expression above it, so method chains can put one call per line:

  ```brim
  let names = people
      .filter(fn(p: Person) -> bool { return p.age > 30 })
      .map(fn(p: Person) -> string { return p.name })
  ```

To continue a long expression, end the line with the operator:

```brim
let total: i64 = first +
    second +
    third
```

A line that starts with a binary operator is a new statement, which is normally an error.

### 6.2 `let`

`let name: T = expr` evaluates `expr`, which must have type exactly `T`, and binds it to `name` in the current scope. The binding is immutable.

`let name = expr` leaves out the type: the variable gets the type of `expr`. Values whose type cannot be known on their own, such as `[]`, `[:]` and `None`, need the annotation (`E0226`), and so does an integer literal of a type other than `i64` (`let b: u8 = 200`). A call of a function that returns nothing has no value to store (`E0213`).

`let mut name: T = expr` declares a **mutable** variable that may be reassigned. A `mut` variable that is never reassigned produces warning `W0003`.

### 6.3 Assignment

`name = expr` evaluates `expr` and stores it in the variable `name`, which must be declared with `let mut` (`E0218`). Function parameters are never mutable. The value must have the variable's type exactly (`E0206`).

The compound forms `name op= expr` (with `op` one of `+ - * / %`) mean `name = name op expr`, with the same operand rules as the binary operators (section 8.1). For example, `s += "x"` appends to a mutable `string`.

Assignment is a statement, not an expression: it produces no value.

### 6.4 `if` / `else`

`if cond { ... }` runs the block when `cond` is `true`. `cond` must be a `bool` (`E0206`); there is no implicit truthiness, so write `n != 0` instead of `n`. An optional `else { ... }` runs otherwise, and `else if` chains are allowed. Each block opens a new scope.

**`if` as a value.** In an expression, `if cond { a } else { b }` is the value of the branch that runs: `let sign = if n < 0 { -1 } else { 1 }`. The `else` is required (`E0250`), and both branches give values of the same type. A block used as a value ends with an expression (or an `if` or `match` used as a value), after any other statements; a block that ends otherwise is `E0250`, unless it returns or breaks first, in which case it needs no value:

```brim
let n = match text.parse_int() {
    Some(n) => n
    None => {
        print("not a number")
        return
    }
}
```

At the start of a line, `if` and `match` are statements, and their blocks' values are unused.

### 6.5 `while`

`while cond { ... }` evaluates `cond` (which must be a `bool`) and runs the block as long as it is `true`. The condition is evaluated before every iteration.

- `break` leaves the innermost enclosing loop.
- `continue` jumps to the next evaluation of the innermost loop's condition.
- `break` or `continue` outside a loop is error `E0217`.

`loop { ... }` runs the block until a `break` (or `return`); it is the same as `while true { ... }`, and diverges when it contains no `break` for it.

### 6.6 `return`

`return expr` returns from the enclosing function. `expr` must have the function's return type. A plain `return` is allowed only in `void` functions (`E0208`).

### 6.7 Expression statements

Evaluates an expression for its effects. A non-call expression used as a statement (`1 + 2`) produces warning `W0002`. Discarding the result of a call is allowed.

### 6.8 Blocks

`{ ... }` groups statements and opens a new scope.

### 6.9 `for`

```brim
for name in names {
    print(name)
}
for i in 0..10 {
    print(i)
}
```

`for x in list { ... }` runs the body once for every element of the list, in order. The list is evaluated once, before the loop, so changing the variable it came from inside the loop does not change which elements the loop visits. `for i in a..b { ... }` runs it for every integer from `a` up to, but not including, `b`. The loop variable is immutable and lives in the body's scope. `break` and `continue` work as in `while` (`continue` moves on to the next element). Looping over anything else is `E0235`. A `for` loop never diverges.

### 6.10 `match`

```brim
match shape {
    Shape.Circle(r) => print(r)
    Rect(w, h) => {
        print(w * h)
    }
    _ => {}
}
```

`match value { pattern => body ... }` runs the body of the first arm whose pattern matches `value`. A body is a block or a single statement. Arms are separated by newlines or commas. Patterns:

- `_` matches anything.
- A name binds the matched value to a new immutable variable in the arm. A name that is a variant of the matched enum (`None`, `Empty`) is a variant pattern instead, and a capitalized name that is not one is an error (`E0227`, `E0230`), so a misspelled variant never silently matches everything.
- A variant pattern, `Enum.Variant(p, q)` or just `Variant(p, q)`, matches that variant of an enum when each of its values matches the pattern in that position. Patterns nest to any depth: `Some(Shape.Circle(r))`, `Ok(0)`, `Pair(_, Leaf(n))`. The pattern must have exactly as many patterns as the variant has values (`E0228`).
- An integer (possibly negative), string or boolean literal matches that value of an `i64`, `string` or `bool`.
- `p | q | ...` matches when any alternative matches. Every alternative must bind the same names, with the same types (`E0245`).

An arm may have a **guard**, `pattern if condition => body`: the arm only runs when the pattern matches and the condition, a `bool` that can use the pattern's names, is true. Otherwise the next arm is tried.

A pattern must fit the type of the matched value (`E0230`). The arms must be **exhaustive** (`E0229`): together they must match every value, so every variant of an enum (at every depth of nesting), both `true` and `false`, and some arm that matches any integer, string or value of another type. Arms with a guard do not count towards this, since the guard may be false. The error lists examples of values no arm matches. An arm, or an alternative of `|`, that can never match because earlier arms cover everything it matches produces warning `W0004`.

A `match` statement diverges (section 6.11) when it is exhaustive and every arm diverges. In an expression, `match` is a value, like `if` (section 6.4): every arm's body ends with a value of one type, or diverges.

### 6.11 Reachability and returns

A statement **diverges** when control can never continue to the statement after it:

- `return`, `break` and `continue` diverge;
- a block diverges if any statement in it diverges;
- `if`/`else` diverges if both branches diverge; an `if` without `else` never diverges;
- `while true { ... }` diverges if its body contains no `break` for that loop. Any other `while` does not diverge, because its body may run zero times.
- `match` diverges if every arm diverges.

A function with a return type other than `void` must not be able to reach the end of its body: the body must diverge (`E0207`). Statements after a diverging statement are unreachable and produce warning `W0001`.

## 7. Names and scopes

- **Types**: the built-in types, `Option`, `Result`, `List`, `Map`, `Set`, `Box`, and the structs, enums and traits of the module, which are visible everywhere in the module, together with the types it imports (section 9.2). Type parameters are visible in their declaration.
- **Functions and constants** of a module form one namespace, together with the items it imports by name. They are visible everywhere in the module, including before their declaration, so mutual recursion works. Defining two with the same name is `E0202`. Defining a function with a builtin's name is `E0216`.
- **Variables** live in lexical scopes. A function's parameters and its body's top-level statements share one scope. Each nested block opens a new scope.
- Declaring a variable twice in the same scope is `E0203`, and duplicate parameter names are `E0204`. A nested block may **shadow** an outer variable.
- A `let` binding is visible from the statement after it until the end of its block. In `let x: i64 = x`, the `x` on the right refers to an outer `x`.
- Variables are immutable unless declared with `let mut` (section 6.2).
- Variables and functions are separate namespaces. Using a function name as a value is `E0214`, and so is calling a variable.

## 8. Expressions

Operands are evaluated left to right. Function arguments are evaluated left to right before the call.

### 8.1 Operators

| Operator            | Operand types              | Result   | Notes |
|---------------------|----------------------------|----------|-------|
| `+` `-` `*`         | two integers of the same type (`i64`, `i32`, `u8`, `u64`) | that type | a result outside the type is a run-time error |
| `/`                 | two integers of the same type | that type | truncates toward zero; division by zero and `i64::MIN / -1` are run-time errors |
| `%`                 | two integers of the same type | that type | result has the sign of the left operand; `% 0` is a run-time error |
| `+` `-` `*` `/`     | `f64`, `f64`               | `f64`    | IEEE 754: never a run-time error; `1.0 / 0.0` is `inf`, `0.0 / 0.0` is `NaN` |
| `%`                 | `f64`, `f64`               | `f64`    | remainder with the sign of the left operand (like C's `fmod`) |
| `+`                 | `string`, `string`         | `string` | concatenation |
| `<` `<=` `>` `>=`   | two integers of the same type, two `f64`, two `string`, or two values of a type parameter bounded by `Ord` | `bool`   | strings compare byte by byte; comparisons with `NaN` are false |
| `==` `!=`           | two values of the same type that implements `Eq`: numbers, `bool`, `string`, and structs, enums, tuples, lists, maps and sets of such values | `bool` | strings compare by content; `NaN == NaN` is false and `NaN != NaN` true; other values compare structurally (section 5.8) |
| `&&` `\|\|`         | `bool`, `bool`             | `bool`   | short-circuit: the right side is evaluated only if needed |
| unary `-`           | `i64`, `i32` or `f64`      | same     | `-i64::MIN` is a run-time error; `u8` and `u64` cannot be negated |
| unary `!`           | `bool`                     | `bool`   | |

Any other combination is error `E0209`; in particular `i64` and `f64` never mix (`1.5 * 2` is an error, `1.5 * 2.0` is not).

### 8.2 Calls

`f(a, b)` calls the function `f`. The number of arguments must match the number of parameters (`E0205`), and each argument must have exactly its parameter's type (`E0206`). The call's type is `f`'s return type. A call to a `void` function cannot be used as a value.

## 9. Program structure and execution

- A program is one source file, its **root module**, plus the modules it imports (section 9.2).
- The program must define `fn main()` with no parameters and no return type, or `-> void` (`E0210`, `E0211`).
- Execution evaluates `main`'s body. The program ends when `main` returns or a run-time error occurs, also while tasks it started still run (section 9.7).

### 9.1 Run-time errors

These stop the program immediately, after the output produced so far has been flushed:

| Code    | Condition |
|---------|-----------|
| `R0001` | integer overflow in `+`, `-`, `*`, `/`, `%` or unary `-` |
| `R0002` | division or remainder by zero |
| `R0003` | call depth limit exceeded (10,000 nested calls, in the interpreter and in native executables) |
| `R0004` | writing program output failed |
| `R0005` | a list index outside the list |
| `R0006` | a map key that is not in the map |
| `R0007` | a string slice outside the string or not on a character boundary |
| `R0008` | a failed `assert` or `assert_eq` |
| `R0009` | the step limit an embedder of the interpreter set (never in `brim run` or native executables) |
| `R0010` | `to_int` of NaN or of a number outside the `i64` range |
| `R0011` | a call to an `extern fn` in the interpreter |
| `R0012` | `join` of a task that was already joined |
| `R0013` | starting a task where there are no threads (the interpreter compiled to WebAssembly) |

Integers never wrap silently. `docs/error-handling.md` lists every code with an example.

### 9.2 Modules

```brim
// main.brim
import geometry
import text.format.{pad, Report}

fn main() {
    print(geometry.area(geometry.square(3)))
}

// geometry.brim
pub fn square(side: i64) -> Shape { ... }
fn helper() { ... }          // private to geometry
```

Every `.brim` file is a module. `import a.b` loads the module in `a/b.brim`, relative to the directory of the root file (`src/` in a project).

- `import a.b` makes the module usable as `b`: `b.function(...)`, `b.Type`, `b.Type { ... }`, `b.Enum.Variant(...)`, `b.Type.method(...)`. `import a.b as c` uses the name `c` instead.
- `import a.b.{X, y}` brings the named items (functions, types and constants) into the module's scope under their own names.
- `pub import a.b.{X, y}` also **re-exports** them: modules that import this one can use them as `this.X` or import them by name, as if they were declared here. Only item lists can be re-exported.
- A module's functions, structs, enums, traits and constants are **private** unless declared `pub`; using a private item of another module is `E0241`. Methods are private unless declared `pub fn` inside the `impl` block (methods of `impl Trait for Type` are as visible as the trait).
- **Fields** are private too unless declared `pub name: type`: outside the struct's module, reading, assigning or giving a private field (so creating the struct with a literal) is `E0241`. A struct with private fields is made by functions of its module.
- Methods can only be added in the module that declares the type (`E0237`).
- A missing module or item is `E0242`; importing two modules under the same name, or an item whose name is already taken, is `E0243`.
- Modules may import each other in cycles. Every module is loaded once.
- `main` must be in the root module. Function names of other modules appear qualified (`geometry.area`) in messages and run-time call stacks.
- Modules can also come from other packages: see `[dependencies]` in Appendix A.
- `import std.x` imports a module of the standard library, which is built into the compiler (section 10.2). The name `std` is reserved for it.

### 9.3 Tests

```brim
fn add(a: i64, b: i64) -> i64 {
    return a + b
}

fn test_add() {
    assert_eq(add(2, 3), 5)
}
```

`brim test` runs the program's tests: the functions whose names start with `test_`, declared outside `impl` blocks, in any module. A test takes no parameters and returns nothing (`E0244` otherwise). It passes when it returns and fails when it stops with a run-time error, such as a failed `assert_eq`, or calls `exit`. For `brim test`, `main` is optional; it is never run. Other commands ignore tests, so a program still needs `main`.

### 9.4 API types for TypeScript

`brim gen ts` writes TypeScript declarations for the `pub` structs and enums of a program's own modules and the types they use, so a web frontend and a Brim backend share one definition of their API. The declarations describe values as JSON:

| Brim | TypeScript |
|------|------------|
| `i64`, `i32`, `u8`, `u64`, `bool`, `string` | `number`, `boolean`, `string` |
| `List[T]` | `T[]` |
| `Map[string, V]`, `Map[i64, V]`, `Map[bool, V]` | `Record<string, V>`, `Record<number, V>`, `Partial<Record<"true" \| "false", V>>` |
| `Option[T]` | `T \| null` |
| `Result[T, E]` | `{ kind: "Ok"; value: T } \| { kind: "Err"; value: E }` |
| `struct P { x: i64 }` | `interface P { x: number }` |
| enum without payloads | a union of the variant names: `"Low" \| "High"` |
| enum with payloads | `{ kind: "Name" }`, `{ kind: "Name"; value: T }` or `{ kind: "Name"; value: [T, U] }` per variant |

Generic types stay generic (`Page[T]` becomes `Page<T>`). Exported types need different names, even when they come from different modules (`G0001`). `examples/api.brim` and `examples/api.ts` show the result. `json.encode` and `json.decode` (section 10.2) write and read values in exactly these shapes; map keys of any integer type are `Record<number, V>`.

### 9.5 API functions and an Angular client

`serve_api(module, request)`, a built-in function, answers an `http.Request` with an `http.Response` by calling one of the `pub fn`s of `module`, an imported module: its *API functions*. A call is `POST /api/<name>` with the parameters as a JSON object (`{"text": "milk"}` for `fn add_note(text: string)`; an empty body is `{}`). Parameters and results are read and written with `json.decode` and `json.encode`, so they must implement `Data`, and API functions cannot be generic or methods (`E0255`).

| Request | Response |
|---------|----------|
| the function returns `value`, or `Ok(value)` | 200, `value` as JSON (`null` for a function without a result) |
| the function returns `Err(error)` | 422, `error` as JSON |
| a parameter does not fit | 400, `{"error": "at $.text: expected a string, found a number"}`; a missing parameter is `null` |
| the body is not a JSON object | 400, `{"error": "..."}` |
| no API function has the name | 404, `{"error": "no API function \"name\""}` |
| another method than `POST` | 405, with `allow: POST` |
| a path that does not start with `/api/` | 404 |

API functions share no state: they keep it in files or a database. `http.new_request(method, path, body)` makes a request without a connection, for testing a handler.

`brim gen ts --angular` adds an Angular service per module served with `serve_api`: `notes_api` becomes `NotesApiClient`, `@Injectable({ providedIn: 'root' })`, with a method per API function in camel case. `add_note(text: string) -> Result[Note, NoteError]` becomes `addNote(text: string): Observable<Note>`, which posts `{ text }`; a 422 response arrives as an `HttpErrorResponse` whose `error` is the `NoteError`. The injection token `API_BASE_URL` (default `""`, the page's own origin) says where the server is. Without a module served with `serve_api`, `--angular` is an error (`G0002`). `examples/http/api` shows a service and its generated `client.ts`.

### 9.6 Calling C

An `extern fn` declares a C function, without a body; calls to it call the C function of that name:

```brim
extern fn cos(x: f64) -> f64
extern fn crc32(data: string, len: i64) -> u64
```

Parameters and the result have types C has too, and are passed as C expects (`E0260` for others, for generics, for `self` and for an `extern fn main`):

| Brim | C |
|------|---|
| `i64`, `i32`, `u8`, `u64` | `int64_t`, `int32_t`, `uint8_t`, `uint64_t` |
| `f64` | `double` |
| `bool` | `bool` |
| `string` | `const char *`: a NUL-terminated copy for the call (text after a NUL inside the string is not seen); a returned string is copied, and `NULL` is `""`. The C side keeps what it returns. |
| no result | `void` |

The C code comes from the files in `[build] c-sources` (compiled with the C compiler that links the program, where they are, so `#include "..."` works) and the libraries in `[build] link` (`-l<name>`), for every package of the program, or from the C library. Native builds only: the interpreter stops at a call with `R0011`. `brim run` therefore builds a program that has `extern fn`s and runs the executable, when the native backend is available. `brim test` runs in the interpreter, so tests cannot call C yet.

### 9.7 Tasks

`std.task` runs functions at the same time, each on a thread of its own:

```brim
import std.task

fn main() {
    let a = task.spawn(fn() -> i64 { return 6 * 7 })
    let lines: task.Channel[string] = task.channel()
    task.detach(fn() {
        lines.send("hello")
        lines.close()
    })
    print(a.join())             // 42
    print(lines.receive())      // Some("hello")
    print(lines.receive())      // None
}
```

- `spawn(f: fn() -> T) -> Task[T]` starts a task; `Task[T].join() -> T` waits for it and returns its result. A task is joined once: joining it again is the run-time error `R0012`. `detach(f: fn())` starts a task that is never joined.
- `channel[T]() -> Channel[T]` is a channel without a size limit, `bounded[T](capacity)` one whose `send` waits while `capacity` values are queued. `Channel[T].send(value) -> bool` queues a value (`false` once the channel is closed), `receive() -> Option[T]` waits for the next one (`None` once the channel is closed and every value sent before was received), `close()` ends it. Copies of a channel, in a `fn`, a struct or another channel, refer to the same channel.
- Tasks share no changeable memory. Every value is a value: a `fn` copies what it captures, and a list, map or struct a task changes is its own copy, made when it changes it. So no task sees another task change something, and there are no data races. Values passed between tasks are shared until one side changes them, as within a task.
- A run-time error in any task stops the program, with that task's call stack; `exit` in any task ends the program. The program ends when `main` returns, while detached tasks run or wait.
- `print` writes a line at a time: the lines of different tasks do not mix, in an order that depends on how the tasks run.
- A task waiting for a value no task sends waits forever: close a channel to end a loop over `receive`.

## 10. Builtins

Builtins are always in scope and cannot be redefined.

| Builtin | Forms                                             | Effect |
|---------|---------------------------------------------------|--------|
| `print` | `print(string)`, `print(i64)`, `print(f64)`, `print(bool)`, and structs, enums, lists and maps (section 5.8) | writes the value and a newline to standard output |
| `eprint` | `eprint(string)` | writes the text and a newline to standard error |
| `to_string` | `to_string(i64) -> string`, `to_string(f64) -> string`, `to_string(bool) -> string`, and structs, enums, lists and maps | the decimal representation of a number, `"true"`/`"false"`, or the text form of section 5.8 |
| `to_float` | `to_float(i64) -> f64`, and the other integer types | the nearest `f64` |
| `abs` | `abs(i64) -> i64`, `abs(f64) -> f64` | the absolute value; `abs` of `i64::MIN` is run-time error `R0001` |
| `sqrt`, `floor`, `ceil`, `round` | `(f64) -> f64` | IEEE 754; `round` rounds halves away from zero, `sqrt` of a negative number is NaN |
| `pow` | `pow(i64, i64) -> i64`, `pow(f64, f64) -> f64` | `a` to the power `b`. On integers a result outside `i64` is `R0001`, and a negative power divides, rounding toward zero (`pow(2, -1)` is 0, `pow(0, -1)` is `R0001`) |
| `min`, `max` | `min[T: Ord](a: T, b: T) -> T` | from the prelude; `a` when the two are equal |
| `clamp` | `clamp[T: Ord](x: T, low: T, high: T) -> T` | from the prelude: `min(max(x, low), high)` |
| `to_int` | `to_int(f64) -> i64`, `to_int(i32) -> i64`, `to_int(u8) -> i64`, `to_int(u64) -> i64` | rounds an `f64` toward zero; NaN and numbers outside the `i64` range are run-time error `R0010`; a `u64` above the `i64` range is `R0001` |
| `to_i32`, `to_u8`, `to_u64` | `to_i32(x) -> i32` and so on, for any integer `x` | the same number in another integer type; `R0001` if it does not fit |
| `string_from_bytes` | `string_from_bytes(List[u8]) -> Option[string]` | the text of UTF-8 bytes, or `None` if they are not valid UTF-8 |
| `from_code_point` | `from_code_point(i64) -> Option[string]` | the character with a Unicode code point, or `None` for a surrogate or a number outside `0..=10FFFF` (hex) |
| `read_line` | `read_line() -> Option[string]` | the next line of standard input without its line ending (`\n` or `\r\n`); `None` at the end of input |
| `read_file` | `read_file(path: string) -> Result[string, string]` | a file's contents; `Err` with a message if it cannot be read or is not UTF-8 |
| `write_file` | `write_file(path: string, text: string) -> Result[i64, string]` | creates or replaces a file; `Ok` with the number of bytes written |
| `args` | `args() -> List[string]` | the program's command-line arguments, without the program name |
| `env` | `env(name: string) -> Option[string]` | an environment variable |
| `exit` | `exit(code: i64)` | ends the program with an exit code (its low 8 bits); a statement calling `exit` diverges |
| `assert` | `assert(bool)` | stops the program with run-time error `R0008` if the condition is false |
| `assert_eq` | `assert_eq(i64, i64)`, `assert_eq(bool, bool)`, `assert_eq(string, string)` | stops the program with `R0008` if the values differ; the message shows both |

The prelude also has the constants `PI` and `E` (`f64`). Its functions and constants can be used by name everywhere, unless a module defines its own with the same name. A program may also define its own `abs`, `sqrt`, `floor`, `ceil`, `round` or `pow`, which it then uses instead of the builtin; redefining other builtins is `E0216`.

The prelude's struct `StringBuilder` builds a string from pieces: `StringBuilder.new()`, `b.add(text)`, `b.add_line(text)`, `b.is_empty()` and `b.build()`. Joining with `+` in a loop copies the text so far every time; a builder joins once.

An `f64` is written with the fewest digits that read back as exactly the same number: `0.1`, `0.30000000000000004`. Numbers from `0.00001` up to (not including) `1e16` are written as decimals, with `.0` when whole (`2.0`); others in scientific notation (`1e16`, `1.5e-7`). The special values are `NaN`, `inf` and `-inf`, and negative zero is `-0.0`.

A builtin selects its form by the argument's type. User functions cannot be overloaded, and builtins cannot be redefined (`E0216`).

File error messages have the form `cannot read file "PATH": REASON`, where the reason is `file not found`, `permission denied`, `is a directory`, `file is not valid UTF-8` or `input/output error`. Relative paths are relative to the current directory.

### 10.1 String methods

Positions and lengths count **bytes** of the UTF-8 text.

| Method | Result | |
|--------|--------|---|
| `len()` | `i64` | the length in bytes |
| `is_empty()` | `bool` | |
| `contains(t)`, `starts_with(t)`, `ends_with(t)` | `bool` | |
| `find(t)` | `Option[i64]` | the position of the first `t` |
| `slice(start, end)` | `string` | bytes `start..end`; outside the string or not on a character boundary is run-time error `R0007` |
| `trim()`, `trim_start()`, `trim_end()` | `string` | without spaces, tabs, line feeds, carriage returns, vertical tabs and form feeds |
| `to_upper()`, `to_lower()` | `string` | ASCII letters only |
| `replace(from, to)` | `string` | every non-overlapping `from`; an empty `from` matches before every character and at the end |
| `repeat(n)` | `string` | empty for `n <= 0` |
| `split(sep)` | `List[string]` | the parts between separators (`"a,,b"` gives three parts); an empty separator splits into characters |
| `lines()` | `List[string]` | without line endings; a final line ending does not start another line |
| `chars()` | `List[string]` | each character as a string |
| `char_count()` | `i64` | the number of characters (code points); `len()` counts bytes |
| `code_points()` | `List[i64]` | the Unicode code point of each character |
| `parse_int()` | `Option[i64]` | a decimal integer with an optional `+` or `-`, and nothing else |
| `parse_float()` | `Option[f64]` | a decimal number with an optional sign, fraction and exponent (`-1.5e3`, `12`); the nearest `f64`, or `inf` for numbers too large. `NaN`, `inf`, `1.` and `.5` are not accepted |

Strings can be compared with `<`, `<=`, `>` and `>=`, byte by byte.

### 10.2 Standard library modules

Some of the standard library is written in Brim and built into the compiler. Its modules are imported like any other: `import std.http` makes `http.get(...)`, `http.Request` and so on available.

| Module | Contents |
|--------|----------|
| `std.net` | TCP: `listen(host, port) -> Result[Listener, string]`, `connect(host, port) -> Result[Conn, string]`; `Listener.accept()`, `next_ready(idle_millis)` (the next connection with something to read: a new one or a kept one), `keep(conn)` (give a connection back for `next_ready`, at most 256), `port()`, `close()`; `closed()`, a connection that is not open; `Conn.read_line()`, `read_until(delimiter)`, `read_exact(count)`, `read_all()`, `write(text)`, `read_bytes(count) -> Result[List[u8], string]` and `write_bytes(bytes)` for binary protocols, `set_timeout(millis)`, `port()`, `close()` |
| `std.time` | `now() -> Time` (the system clock), `from_unix_millis`, `from_unix_seconds`; `Time.iso()` (`2026-10-15T08:30:01.123Z`), `date()`, `http_date()`, `parts() -> DateTime` (year, month, day, hour, minute, second, millisecond, weekday), `unix_seconds()`, `add_millis`, `add_seconds`; `millis()` (a monotonic clock for durations), `sleep(millis)`. All in UTC |
| `std.log` | `info(message)`, `warn(message)`, `error(message)`: a line on standard error with the time and level |
| `std.fs` | `exists`, `is_dir`, `is_file`; `list(dir) -> Result[List[string], string]` (sorted names); `create_dir(path)` (with parents; `Ok(false)` if it existed), `remove(path)` (a file or empty directory; `Ok(false)` if there was none), `remove_all(path)`, `rename(from, to)`, each `-> Result[bool, string]`; `size(path) -> Result[i64, string]`, `modified(path) -> Result[time.Time, string]`; `read`, `write` and `append(path, text)`. Errors are messages such as `cannot list directory "out": not found`; the reasons are `not found`, `permission denied`, `already exists`, `directory not empty`, `not a directory`, `is a directory` and `input/output error` |
| `std.process` | `run(cmd, args: List[string]) -> Result[Output, string]` runs a program and waits for it; `Output` has `code: i64` (128 plus the signal for a program a signal stopped), `stdout` and `stderr` (strings) and `success()`. The program is looked up in `PATH` unless its name contains `/`, gets no input and runs in the working directory. `Err` only when it cannot start (`cannot run "x": not found`) or writes text that is not UTF-8. `output(cmd, args) -> Result[string, string]` is the standard output of a program that exits with 0, and otherwise an error with its standard error |
| `std.crypto` | `sha256(bytes)`, `sha256_hex(text)`, `hmac_sha256(key, message)`, `pbkdf2_sha256(password, salt, iterations, length)`, `md5(bytes)`, each on `List[u8]`; `random_bytes(n)` from the operating system's secure source (`/dev/urandom`); `hex(bytes)`; `equal(a, b)`, which takes as long whatever the first difference |
| `std.base64` | `encode(bytes) -> string` and `decode(text) -> Result[List[u8], string]` (RFC 4648, standard alphabet, `=` padding; spaces and line breaks are skipped), `encode_text(text)` and `decode_text(text)` for strings |
| `std.postgres` | A PostgreSQL client over TCP, without TLS. `connect(url) -> Result[Client, string]` for `postgres://user:password@host:port/database` (port 5432 and the user's database unless given), `connect_to(host, port, user, password, database)`; `Client.query(sql, params) -> Result[Rows, string]`, `execute(sql, params) -> Result[i64, string]` (the rows changed), `query_as[T: Data](sql, params) -> Result[List[T], string]` (each row as an object read with `json.from_json`), `transaction(work)` (`BEGIN`, then `COMMIT` if `work` returns `Ok` and `ROLLBACK` if `Err`), `close()`. Parameters are `$1`, `$2`, ... and a `List[Value]`: `null()`, `int(n)`, `float(x)`, `text(s)`, `bool(b)`, `json_value(v)`, sent separately from the SQL. `Rows` has `columns`, `rows` and `affected`; a `Row` has `get(column) -> Option[string]`, `text`, `int`, `float` and `bool` (`Result`s), `is_null` and `to_json()`, which writes `bool`, integer, `float8`, `numeric` and `json`/`jsonb` columns as themselves. Logins use SCRAM-SHA-256, MD5 or a plain password, as the server asks. Errors: `postgres: ERROR: relation "x" does not exist [42P01]` |
| `std.task` | Tasks and channels (section 9.7): `spawn(f) -> Task[T]`, `Task.join()`, `detach(f)`; `channel[T]()`, `bounded[T](capacity)`; `Channel.send(value) -> bool`, `receive() -> Option[T]`, `close()` |
| `std.random` | `int(low, high)` (from `low` up to, not including, `high`; `low` when `high <= low`), `float()` (0 up to 1), `bool()`, `choice(list) -> Option[T]`, `shuffle(list) -> List[T]`, `seed(n)`. The generator is xoshiro256**: fast, not for passwords or keys. After `seed(n)` the numbers are the same on every run, in the interpreter and native code alike; without it they come from the operating system's randomness |
| `std.json` | `enum Json { Null, Bool(bool), Number(string), Str(string), Array(List[Json]), Object(Map[string, Json]) }`; `parse(text) -> Result[Json, string]`, `stringify(value)`, `pretty(value)`, `quote(text)`; building: `null()`, `from_bool`, `from_int`, `from_float`, `from_string`, `array`, `object`; reading: `Json.get(key)`, `at(index)`, `as_string()`, `as_int()`, `as_float()`, `as_bool()`, `as_array()`, `as_object()`, `is_null()`, `kind()`; typed values (`T: Data`): `encode(value) -> string`, `decode(text) -> Result[T, string]`, `to_json(value) -> Json`, `from_json(json) -> Result[T, string]` |
| `std.http` | HTTP/1.1. Server: `serve(host, port) -> Result[Server, string]`, `Server.next() -> Result[Request, string]`, `Request.respond(Response)`, `Request.header(name)`, `Request.param(name)`. Client: `get(url)`, `post(url, content_type, body)`, `request(method, url, headers, body)`, each `-> Result[Response, string]`. Responses: `text`, `json`, `html`, `response`, `redirect`, `not_found`, `Response.with_header`, `header`, `ok`. URLs: `encode`, `decode`. `new_request(method, path, body)` for tests; the built-in `serve_api(module, request)` serves API functions (section 9.5) |

Network data is text: what a connection reads must be valid UTF-8. Network errors are messages such as `cannot connect to example.com:80: connection refused`; the reasons are `connection refused`, `connection reset`, `connection closed`, `timed out`, `host not found`, `address in use`, `address not available`, `permission denied`, `invalid port` and `network error`, and for `https://` also `certificate not trusted`, `TLS handshake failed` and `invalid URL`.

The HTTP server handles a request at a time; a task per request (`task.detach`, section 9.7) handles them at the same time, and `Server.next()` and `Request.respond` may be called from different tasks. It keeps a connection open after the response for the client's next request (HTTP keep-alive) when the request is HTTP/1.1 without `connection: close`, or HTTP/1.0 with `connection: keep-alive`, and the response has no `connection: close` header; the response says `connection: keep-alive` or `connection: close`. `Server.next()` waits for a new connection and for the kept ones at once, so a quiet client keeps no one else out; a kept connection that stays quiet for five seconds, or that the client closes, is closed. It protects itself against clients that send too much: request heads over 64 KiB get 431, bodies over the limit (10 MiB, or `Server.with_max_body(bytes)`) get 413, and a client gets ten seconds to send its request. `Conn.read_until_max(delimiter, max)` gives the same protection to programs that use `std.net` directly. Header names are kept in lower case; query parameters and paths are percent-decoded. The client reads bodies with a `content-length`, `chunked` bodies and bodies that last until the connection closes; `https://` URLs go through libcurl, which checks certificates; a native program that uses the HTTP client links with libcurl (`-lcurl`, so building it needs libcurl's development files on Linux), and other programs, servers included, do not. Only the functions a program can reach from `main` (or its tests) are compiled. Redirects are not followed: the 3xx response is returned.

`std.json` follows RFC 8259. Numbers keep the text they were written with (`Number("-2.5e3")`), so no precision is lost; `as_int` reads whole numbers and `as_float` any number. `from_float` writes NaN and infinities, which JSON cannot hold, as `null`. Object members keep their order, and a repeated key keeps its last value. Parsing rejects lone surrogates (`"\ud83d"`), which are not characters, and values nested more than 128 deep. Parse errors name the problem and the character position: `expected ':' at character 6`.

`encode` and `decode` use the shapes `brim gen ts` declares: numbers, booleans and strings as themselves (a NaN or infinite `f64` as `null`); a struct as an object with every field; `Option` as its value or `null` (a missing member decodes as `None`); an enum whose variants have no payloads as the variant's name (`"Red"`), other enums, `Result` among them, as `{"kind": "Circle", "value": 1.5}` with an array for several payload values and no `"value"` for none; `Box[T]` as the value; tuples, lists and sets as arrays; maps as objects, with integer and `bool` keys written as text (`{"1": "one"}`). `decode` also accepts a variant name alone for a variant without a payload. Its errors give the path to the value: `at $.tags[1]: expected a string, found a number`, `at $[0]: Shape has no variant "Triangle"`, `at $.n: 300 does not fit in u8`. Decoding a `u64` accepts values up to the largest `i64`. The type to decode comes from the expected type, which may come through `?`: `let note: Note = json.decode(text)?`.

The standard library modules use builtins whose names start with `__` (`__net_listen`, ...). Programs cannot call them (`E0201`).

## 11. Design decisions

The questions that were open before 1.0, and how 1.0 answers them. Each answer is part of the stable language (see `docs/releasing.md`); where 1.0 leaves something out, adding it later does not break programs written for 1.0.

1. **Type inference.** `let` takes the type of its value. Types are not inferred from later uses (`let mut xs = []` followed by `xs.push(1)` needs `List[i64]`): a declaration says everything about the variable. Type arguments of calls and literals are inferred from arguments and the expected type (section 5.3).
2. **`if` and `match` as values.** Both are expressions (section 6.4); at the start of a line they are statements.
3. **Integer types.** `i64`, `i32`, `u8` and `u64` (section 5). `i64` is the default. More widths (`i8`, `i16`, `u16`, `u32`) may be added as new types.
4. **Overflow.** Integer operations are checked in every build, debug and release, in the interpreter and in native code: a result that does not fit is `R0001`. Wrapping arithmetic, if it comes, will be functions with their own names, never a change to the operators.
5. **Strings.** Immutable UTF-8 text, reference-counted. Positions and lengths count bytes (section 10.1); `chars`, `char_count` and `code_points` work on characters. Case conversion covers ASCII letters; full Unicode case mapping and grapheme clusters belong in a library.
6. **Errors.** Expected failures are `Result` values, passed on with `?`, which converts between error enums (section 5.4). Bugs (overflow, an index out of range, a failed assertion) are run-time errors (`R` codes) that stop the program. There are no exceptions and no catching of run-time errors.
7. **Statement termination.** A newline ends a statement, except after an operator, inside brackets and before a line that starts with `.` (section 6.1).
8. **`main`.** `fn main()` takes nothing and returns nothing; arguments come from `args()` and the exit code from `exit`. A `main` returning `Result` may be added later.
9. **Identifiers.** ASCII letters, digits and `_`. Unicode identifiers may be allowed later.
10. **Modules.** A file is a module; items and fields are private unless `pub`; `pub import` re-exports named items. Packages come from the registry by version, from directories or from git (Appendix A).
11. **Loops.** `while`, `loop` and `for` over lists, sets and ranges, with `break` and `continue` for the innermost loop. Labeled loops are not in 1.0.
12. **Struct literals.** Every field is written out: `Point { x: x, y: y }`. Shorthand and default values are not in 1.0.
13. **Patterns.** Literals, names, `_`, variants, tuples, `|` and guards, nested to any depth, checked for exhaustiveness (section 6.10). Struct patterns, ranges and `name @ pattern` are not in 1.0.
14. **Traits.** Methods with `self`, bounds on type parameters, and the built-in traits `Eq`, `Show`, `Ord`, `Key` and `Data` (section 5.11). Default method bodies, static trait methods, traits for built-in types and trait objects are not in 1.0.
15. **Memory.** Automatic reference counting with copy-on-write, for every value. Values cannot refer to themselves except through `Box`, `List` and `Map`, which hold copies, so there are no cycles to collect and no garbage collector.
16. **Concurrency.** Tasks on threads, joined or detached, with channels between them (section 9.7). There is no shared changeable memory, so there are no data races; there is no `async`/`await`.

## 12. Not in 1.0

These are not part of the language. The reserved words `async`, `await`, `type` and `use` keep room for some of them; using one is `E0101`.

- Type aliases (`type`), and the items of section 11 that 1.0 leaves out: labeled loops, struct patterns, field shorthand, default method bodies, trait objects, a `main` that returns `Result`, Unicode identifiers.
- `select` over several channels, `receive` with a timeout, and shared mutable state between tasks.
- Macros, reflection, operator overloading, and an ownership or borrowing system.

`docs/roadmap.md` says what comes after 1.0.

## Appendix A. The `.brimproj` manifest

Brim projects are described by a `.brimproj` file, in TOML. Its format is part of the stable interface (see `docs/releasing.md`). The lock file next to it is `.brimlock`. Before 1.0 the two were called `brim.toml` and `brim.lock`: a project with a `brim.toml` and no `.brimproj` still works, with a warning to rename it, and a `brim.lock` is read and replaced by a `.brimlock` the next time the lock file is written.

```toml
[package]
name = "hello"          # required: lowercase letter, then [a-z0-9_-], max 64 chars
version = "0.1.0"       # required: MAJOR.MINOR.PATCH with an optional -prerelease
edition = "2026"        # required: language edition; only "2026" exists
description = "..."     # optional; for the registry
license = "MIT"         # optional; an SPDX expression, for the registry
repository = "https://..." # optional; for the registry

[build]                 # optional
entry = "src/main.brim" # optional; default "src/main.brim"; relative, inside the package
lib = "src/lib.brim"    # optional; default "src/lib.brim": the library root
c-sources = ["c/glue.c"] # optional; C files for `extern fn`s (see 9.6)
link = ["z"]            # optional; libraries for `extern fn`s: -lz

[dependencies]          # optional
router = "1.2"          # from the registry
geometry = { path = "../geometry" }
shapes = { path = "../shapes", version = "0.3" }
http = { git = "https://example.com/http.git", tag = "v1.0.0" }

[workspace]             # optional; [package] may then be left out
members = ["apps/*", "shared"]
```

- Unknown tables and keys are errors, to catch typos.
- A dependency comes from the package registry (a version requirement: `router = "1.2"` or `router = { version = "1.2" }`; see `docs/registry.md` for the forms), from a directory with a `.brimproj` (`path`, relative to this manifest), or from a git repository (`git`, with at most one of `tag`, `branch` and `rev`; without them, the default branch). A `path` dependency may also have a `version`, which `brim publish` keeps; `version` cannot be combined with `git` (`M0006`).
- Every registry package has one version in a project: the newest non-yanked version that meets every requirement on it (`M0014` when none does; `M0013` for a package the registry does not have).
- A dependency's name is the name of its package, and is used in `import`, so it contains no `-` (`M0007`). All packages of a project have different names (`M0009`).
- A package is used as a library through its library root: `import geometry` is the module in `src/lib.brim` of the package `geometry`, and `import geometry.shapes` the module in `src/shapes.brim` next to it. Inside the package, its modules import each other as usual (`import shapes`). The project's own modules cannot have the name of a dependency.
- A package without `src/main.brim` but with a library root is a library: `brim check` and `brim test` work on its library root.
- `brim test` runs only the tests of the project's own modules.
- **`.brimlock`.** The first build writes the commit of every git dependency and the version and checksum of every registry dependency to `.brimlock`, next to `.brimproj` (at the root of a workspace); later builds use those, without the network when they are cached. `brim update` picks the newest commits and versions. Commit `.brimlock` for programs, so every build uses the same versions. Git repositories are cached in `$BRIM_HOME/git` (by default `~/.brim/git`) and only fetched when a commit is missing.
- **Workspaces.** `[workspace] members` lists member directories relative to the manifest; a last component `*` stands for every subdirectory with a `.brimproj` (`M0012` for invalid members). The manifest may also have a `[package]`, which is then a member too. Members share the root's `.brimlock`. At the root, `check`, `test`, `build` and `doc` work on every member; `-p NAME` picks one, as does running in a member's directory.
- `brim run`, `brim check`, `brim build` and `brim test` without a path use the nearest `.brimproj` in the current directory or its parents.
- `brim fmt` formats the project's source files (all `.brim` files under the manifest's directory, except in `target`).
- `brim new PATH` creates a project: a `.brimproj`, a `src/main.brim` with a test, and a `.gitignore`.
