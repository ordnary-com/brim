use super::*;
use crate::checked::{CheckedExprKind, CheckedStmt};
use brim_diagnostics::{Severity, SourceFile};

fn run(src: &str) -> (Option<CheckedProgram>, Vec<Diagnostic>) {
    let (program, diags) = brim_parser::parse(src);
    assert!(diags.is_empty(), "parse errors: {diags:#?}");
    check(&program)
}

fn ok(src: &str) -> CheckedProgram {
    let (checked, diags) = run(src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    checked.unwrap()
}

/// `(code, message)` of every diagnostic.
fn diags(src: &str) -> Vec<(&'static str, String)> {
    run(src)
        .1
        .into_iter()
        .map(|d| (d.code.map_or("", |c| c.0), d.message))
        .collect()
}

fn codes(src: &str) -> Vec<&'static str> {
    diags(src).into_iter().map(|(c, _)| c).collect()
}

/// Wraps statements in a `main` function.
fn main_with(body: &str) -> String {
    format!("fn main() {{\n{body}\n}}\n")
}

#[test]
fn accepts_the_spec_examples() {
    ok(r#"
fn main() {
    let name: string = "Ordnary"
    let version: string = "0.1.0"

    print("Hello from " + name)
    print("Brim version: " + version)
}
"#);
    let program = ok(r#"
fn add(a: i64, b: i64) -> i64 {
    return a + b
}

fn main() {
    let result: i64 = add(10, 32)
    print(result)
}
"#);
    assert_eq!(program.functions.len(), 2);
    assert_eq!(program.functions[program.entry.unwrap().0].name, "main");
    assert_eq!(program.functions[0].param_count, 2);
    assert_eq!(program.functions[0].ret, Type::I64);
}

#[test]
fn functions_can_be_used_before_definition_and_recursively() {
    ok("fn main() { print(f(3)) }\nfn f(n: i64) -> i64 { return f(n) }");
}

#[test]
fn print_overloads_resolve_to_runtime_functions() {
    let program = ok(&main_with("print(1)\nprint(true)\nprint(\"s\")"));
    let runtimes: Vec<_> = program.functions[0]
        .body
        .iter()
        .map(|s| match s {
            CheckedStmt::Expr(e) => match &e.kind {
                CheckedExprKind::Builtin { runtime, .. } => runtime.symbol(),
                _ => panic!(),
            },
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        runtimes,
        vec![
            "brim_rt_print_i64",
            "brim_rt_print_bool",
            "brim_rt_print_str"
        ]
    );
}

#[test]
fn operator_typing() {
    ok(&main_with(
        r#"
let a: i64 = 1 + 2 * 3 - 4 / 2 % 3
let b: bool = a < 3 && a >= 0 || !(a == 2) && a != 5
let c: string = "x" + "y"
let d: bool = c == "xy" && c != "z" && true == false
let e: i64 = -a
let min: i64 = -9223372036854775808
let max: i64 = 9223372036854775807
print(b)
print(d)
print(e + min + max)
"#,
    ));
}

#[test]
fn error_undefined_variable_with_location() {
    let src = "fn main() {\n    let name: string = \"x\"\n    print(nmae)\n}\n";
    let (_, diags) = run(src);
    assert_eq!(diags.len(), 1);
    let d = &diags[0];
    assert_eq!(d.code.unwrap().0, "E0200");
    assert_eq!(d.message, "undefined variable `nmae`");
    let file = SourceFile::new("t.brim", src);
    let span = d.span().unwrap();
    assert_eq!(file.slice(span), "nmae");
    assert_eq!(file.location(span.start).to_string(), "3:11");
    assert_eq!(
        d.help,
        vec!["a variable with a similar name exists: `name`"]
    );
}

#[test]
fn error_undefined_function() {
    assert_eq!(
        diags(&main_with("prnt(1)")),
        vec![("E0201", "undefined function `prnt`".to_string())]
    );
}

#[test]
fn error_duplicate_declarations() {
    assert_eq!(codes("fn f() {}\nfn f() {}\nfn main() {}"), vec!["E0202"]);
    assert_eq!(
        codes(&main_with("let x: i64 = 1\nlet x: i64 = 2")),
        vec!["E0203"]
    );
    // Shadowing in a nested block is allowed.
    ok(&main_with(
        "let x: i64 = 1\n{\n let x: string = \"s\"\n print(x)\n}\nprint(x)",
    ));
    // A parameter and a top-level `let` share a scope.
    assert_eq!(
        codes("fn f(a: i64) { let a: i64 = 1 }\nfn main() {}"),
        vec!["E0203"]
    );
    assert_eq!(
        codes("fn f(a: i64, a: bool) {}\nfn main() {}"),
        vec!["E0204"]
    );
    assert_eq!(codes("fn print() {}\nfn main() {}"), vec!["E0216"]);
}

#[test]
fn error_argument_count() {
    assert_eq!(
        diags("fn add(a: i64, b: i64) -> i64 { return a + b }\nfn main() { print(add(1)) }"),
        vec![(
            "E0205",
            "function `add` takes 2 arguments but 1 was supplied".to_string()
        )]
    );
    assert_eq!(
        diags(&main_with("print(1, 2)")),
        vec![(
            "E0205",
            "`print` takes 1 argument but 2 were supplied".to_string()
        )]
    );
    assert_eq!(codes(&main_with("print()")), vec!["E0205"]);
}

#[test]
fn error_type_mismatch() {
    assert_eq!(
        diags(&main_with("let x: i64 = \"hello\"")),
        vec![("E0206", "mismatched types".to_string())]
    );
    let (_, d) = run(&main_with("let x: i64 = \"hello\""));
    assert_eq!(
        d[0].primary.as_ref().unwrap().message,
        "expected `i64`, found `string`"
    );
    assert_eq!(
        codes("fn f(s: string) {}\nfn main() { f(1) }"),
        vec!["E0206"]
    );
    assert_eq!(
        codes("fn f() {}\nfn main() { let x: i64 = f() }"),
        vec!["E0206"]
    );
    assert_eq!(codes("fn f() {}\nfn main() { print(f()) }"), vec!["E0206"]);
}

#[test]
fn error_invalid_operators() {
    assert_eq!(
        diags(&main_with("print(\"n = \" + 1)")),
        vec![(
            "E0209",
            "cannot apply `+` to `string` and `i64`".to_string()
        )]
    );
    assert_eq!(codes(&main_with("print(1 && true)")), vec!["E0209"]);
    assert_eq!(codes(&main_with("print(true < false)")), vec!["E0209"]);
    assert!(codes(&main_with("print(\"a\" < \"b\")")).is_empty());
    assert_eq!(codes(&main_with("print(1 == true)")), vec!["E0209"]);
    assert_eq!(codes(&main_with("print(-true)")), vec!["E0209"]);
    assert_eq!(codes(&main_with("print(!1)")), vec!["E0209"]);
}

#[test]
fn errors_do_not_cascade() {
    // One undefined variable used in a long expression produces one error.
    assert_eq!(
        codes(&main_with(
            "let x: i64 = (missing + 1) * 2 - -missing2\nprint(x)"
        )),
        vec!["E0200", "E0200"]
    );
    assert_eq!(
        codes(&main_with("let x: nope = 1\nprint(x + 1)")),
        vec!["E0212"]
    );
}

#[test]
fn error_return_checking() {
    assert_eq!(
        diags("fn f() -> i64 { print(1) }\nfn main() {}"),
        vec![(
            "E0207",
            "function `f` may end without returning a value".to_string()
        )]
    );
    assert_eq!(
        codes("fn f() -> i64 { return }\nfn main() {}"),
        vec!["E0208"]
    );
    assert_eq!(codes("fn f() { return 1 }\nfn main() {}"), vec!["E0208"]);
    assert_eq!(
        codes("fn f() -> i64 { return true }\nfn main() {}"),
        vec!["E0206"]
    );
    // A return inside a nested block counts.
    ok("fn f() -> i64 {\n {\n return 1\n }\n}\nfn main() {}");
}

#[test]
fn error_entry_point() {
    assert_eq!(
        diags("fn helper() {}"),
        vec![("E0210", "no `main` function found".to_string())]
    );
    assert_eq!(codes("fn main(x: i64) {}"), vec!["E0211"]);
    assert_eq!(codes("fn main() -> i64 { return 0 }"), vec!["E0211"]);
    ok("fn main() -> void {}");
}

#[test]
fn error_types() {
    assert_eq!(
        diags(&main_with("let x: int = 1")),
        vec![("E0212", "unknown type `int`".to_string())]
    );
    let (_, d) = run(&main_with("let x: int = 1"));
    assert_eq!(d[0].help, vec!["did you mean `i64`?"]);
    assert_eq!(codes(&main_with("let x: void = 1")), vec!["E0213"]);
    assert_eq!(codes("fn f(x: void) {}\nfn main() {}"), vec!["E0213"]);
}

#[test]
fn error_functions_and_variables_are_not_interchangeable() {
    assert_eq!(
        diags("fn f() {}\nfn main() { let g: i64 = f }"),
        vec![("E0214", "`f` is a function, not a value".to_string())]
    );
    assert_eq!(
        diags(&main_with("let x: i64 = 1\nx(2)")),
        vec![("E0214", "`x` is a variable, not a function".to_string())]
    );
}

#[test]
fn error_integer_range() {
    assert_eq!(
        codes(&main_with("let x: i64 = 9223372036854775808")),
        vec!["E0215"]
    );
    ok(&main_with("let x: i64 = -9223372036854775808\nprint(x)"));
}

#[test]
fn warnings_do_not_block_compilation() {
    let (checked, diags) = run(&main_with("1 + 2\nreturn\nprint(1)\nprint(2)"));
    assert!(checked.is_some());
    let summary: Vec<_> = diags
        .iter()
        .map(|d| (d.severity, d.code.unwrap().0))
        .collect();
    assert_eq!(
        summary,
        vec![(Severity::Warning, "W0002"), (Severity::Warning, "W0001")]
    );
}

#[test]
fn reports_multiple_independent_errors() {
    let src =
        "fn main() {\n let a: i64 = \"s\"\n let b: bool = 1\n undefined()\n}\nfn other() -> i64 {}";
    assert_eq!(codes(src), vec!["E0206", "E0206", "E0201", "E0207"]);
}

#[test]
fn accepts_control_flow_and_mutation() {
    ok(r#"
fn classify(n: i64) -> string {
    if n < 0 {
        return "negative"
    } else if n == 0 {
        return "zero"
    } else {
        return "positive"
    }
}

fn main() {
    let mut i: i64 = 0
    let mut total: i64 = 0
    let mut log: string = ""
    while i < 10 {
        i += 1
        if i % 2 == 0 {
            continue
        }
        total = total + i
        log += to_string(i)
        if total > 20 {
            break
        }
    }
    print(classify(total))
    print(log + " " + to_string(true))
}
"#);
}

#[test]
fn returns_on_all_paths() {
    // Both branches return.
    ok("fn f(b: bool) -> i64 {\n if b { return 1 } else { return 2 }\n}\nfn main() {}");
    // `while true` without `break` never ends.
    ok("fn f() -> i64 {\n while true {\n  return 1\n }\n}\nfn main() {}");
    // A missing `else` means the end can be reached.
    assert_eq!(
        codes("fn f(b: bool) -> i64 {\n if b { return 1 }\n}\nfn main() {}"),
        vec!["E0207"]
    );
    // A `break` makes the loop finish.
    assert_eq!(
        codes("fn f() -> i64 {\n while true {\n  break\n }\n}\nfn main() {}"),
        vec!["E0207"]
    );
    // A normal `while` may not run at all.
    assert_eq!(
        codes("fn f(b: bool) -> i64 {\n while b {\n  return 1\n }\n}\nfn main() {}"),
        vec!["E0207"]
    );
}

#[test]
fn unreachable_after_break_and_branches() {
    assert_eq!(
        codes(&main_with("while true {\n break\n print(1)\n}")),
        vec!["W0001"]
    );
    assert_eq!(
        codes(
            "fn f(b: bool) -> i64 {\n if b { return 1 } else { return 2 }\n print(3)\n}\nfn main() {}"
        ),
        vec!["W0001"]
    );
}

#[test]
fn error_conditions_must_be_bool() {
    let (_, d) = run(&main_with("let x: i64 = 1\nif x {\n}"));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code.unwrap().0, "E0206");
    assert_eq!(d[0].help, vec!["compare explicitly, for example `x != 0`"]);
    assert_eq!(codes(&main_with("while \"yes\" {\n}")), vec!["E0206"]);
}

#[test]
fn error_break_outside_loop() {
    assert_eq!(
        diags(&main_with("break")),
        vec![("E0217", "`break` outside of a loop".to_string())]
    );
    assert_eq!(codes(&main_with("if true { continue }")), vec!["E0217"]);
    // Inside a nested block in a loop is fine.
    ok(&main_with("while true {\n {\n  break\n }\n}"));
}

#[test]
fn error_assignment_rules() {
    assert_eq!(
        diags(&main_with("let x: i64 = 1\nx = 2")),
        vec![(
            "E0218",
            "cannot assign to immutable variable `x`".to_string()
        )]
    );
    let (_, d) = run("fn f(n: i64) {\n n = 1\n}\nfn main() {}");
    assert_eq!(d[0].code.unwrap().0, "E0218");
    assert_eq!(d[0].notes, vec!["function parameters cannot be reassigned"]);
    assert_eq!(
        codes(&main_with("let mut x: i64 = 1\nx = \"s\"")),
        vec!["E0206"]
    );
    assert_eq!(
        codes(&main_with("let mut b: bool = true\nb += 1")),
        vec!["E0209"]
    );
    assert_eq!(codes(&main_with("missing = 1")), vec!["E0200"]);
    ok(&main_with(
        "let mut s: string = \"a\"\ns += \"b\"\nprint(s)",
    ));
}

#[test]
fn warns_about_unneeded_mut() {
    assert_eq!(
        diags(&main_with("let mut x: i64 = 1\nprint(x)")),
        vec![(
            "W0003",
            "variable `x` is declared `mut` but never reassigned".to_string()
        )]
    );
}

#[test]
fn to_string_builtin() {
    ok(&main_with(
        "print(\"n = \" + to_string(42) + \", ok = \" + to_string(true))",
    ));
    assert_eq!(codes(&main_with("print(to_string(\"s\"))")), vec!["E0206"]);
}

const POINT: &str = "struct Point {\n    x: i64\n    y: i64\n}\n";

#[test]
fn checks_structs() {
    let program = ok(&format!(
        "{POINT}struct Line {{ from: Point, to: Point, label: string }}\n\
         fn length(l: Line) -> i64 {{\n    return l.to.x - l.from.x\n}}\n\
         fn main() {{\n\
             let mut l: Line = Line {{ label: \"a\", to: Point {{ x: 3, y: 4 }}, from: Point {{ x: 0, y: 0 }} }}\n\
             l.from.x = 1\n\
             l.to.y += 2\n\
             l = l\n\
             print(length(l))\n\
         }}"
    ));
    assert_eq!(program.structs.len(), 2);
    assert_eq!(program.structs[1].name, "Line");
    let Type::Named(id) = program.structs[1].fields[0].ty else {
        panic!()
    };
    assert_eq!(program.types.get(id).def, Def::Struct(StructId(0)));
    let main = &program.functions[program.entry.unwrap().0];
    let CheckedStmt::Let { init, .. } = &main.body[0] else {
        panic!()
    };
    // Fields stay in source order with their declaration index.
    let CheckedExprKind::StructLit { fields, .. } = &init.kind else {
        panic!()
    };
    let order: Vec<usize> = fields.iter().map(|(i, _)| *i).collect();
    assert_eq!(order, [2, 1, 0]);
    let CheckedStmt::Assign { place, .. } = &main.body[1] else {
        panic!()
    };
    let path: Vec<usize> = place
        .steps
        .iter()
        .map(|s| match s {
            crate::checked::PlaceStep::Field(i) => *i,
            crate::checked::PlaceStep::Index(_) => usize::MAX,
        })
        .collect();
    assert_eq!(path, [0, 0]);
}

#[test]
fn structs_can_be_declared_after_use() {
    ok(
        "fn main() {\n    let b: B = B { a: A { n: 1 } }\n    print(b.a.n)\n}\nstruct B { a: A }\nstruct A { n: i64 }",
    );
}

#[test]
fn struct_declaration_errors() {
    assert_eq!(codes("struct P {}\nstruct P {}\nfn main() {}"), ["E0219"]);
    assert_eq!(codes("struct i64 {}\nfn main() {}"), ["E0219"]);
    assert_eq!(codes("struct P {}\nfn P() {}\nfn main() {}"), ["E0219"]);
    assert_eq!(
        codes("struct P { a: i64, a: bool }\nfn main() {}"),
        ["E0220"]
    );
    assert_eq!(codes("struct P { a: void }\nfn main() {}"), ["E0213"]);
    assert_eq!(
        diags("struct P { a: Pont }\nstruct Point {}\nfn main() {}"),
        [("E0212", "unknown type `Pont`".to_string())]
    );
    // Recursion, direct and through another struct, is reported once.
    assert_eq!(
        diags("struct Node { next: Node }\nfn main() {}"),
        [("E0221", "struct `Node` contains itself".to_string())]
    );
    assert_eq!(
        codes("struct A { b: B }\nstruct B { c: C }\nstruct C { a: A }\nfn main() {}"),
        ["E0221"]
    );
}

#[test]
fn struct_expression_errors() {
    let check = |body: &str| diags(&format!("{POINT}{}", main_with(body)));
    assert_eq!(
        check("    let p: Point = Point { x: 1 }"),
        [("E0222", "missing field `y` in `Point`".to_string())]
    );
    assert_eq!(
        check("    let p: Point = Point { x: 1, y: 2, z: 3 }"),
        [("E0224", "struct `Point` has no field `z`".to_string())]
    );
    assert_eq!(
        check("    let p: Point = Point { x: 1, x: 2, y: 3 }"),
        [("E0223", "field `x` is given more than once".to_string())]
    );
    assert_eq!(
        check("    let p: Point = Point { x: true, y: 2 }"),
        [("E0206", "mismatched types".to_string())]
    );
    assert_eq!(
        check("    let p: Point = Pont { x: 1, y: 2 }"),
        [("E0212", "unknown type `Pont`".to_string())]
    );
    assert_eq!(
        check("    let n: i64 = 5\n    print(n.x)"),
        [("E0224", "`i64` has no fields".to_string())]
    );
    assert_eq!(
        check("    let p: Point = Point { x: 1, y: 2 }\n    p.x = 3"),
        [(
            "E0218",
            "cannot assign to a part of immutable variable `p`".to_string()
        )]
    );
    assert_eq!(
        check("    let p: Point = Point { x: 1, y: 2 }\n    print(p < p)"),
        [(
            "E0209",
            "cannot apply `<` to `Point` and `Point`".to_string()
        )]
    );
    assert_eq!(
        check("    let p: Point = Point { x: 1, y: 2 }\n    print(p == p)\n    print(p)"),
        []
    );
    assert_eq!(
        check("    let p: i64 = Point { x: 1, y: 2 }"),
        [("E0206", "mismatched types".to_string())]
    );
    assert_eq!(check("    print(Point)")[0].0, "E0214");
    assert_eq!(check("    let p: Point = Point(1, 2)")[0].0, "E0201");
}
