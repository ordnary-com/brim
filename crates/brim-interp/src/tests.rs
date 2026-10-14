use super::*;
use brim_diagnostics::SourceFile;

fn compile(src: &str) -> Module {
    let (ast, diags) = brim_parser::parse(src);
    assert!(diags.is_empty(), "{diags:#?}");
    let (checked, diags) = brim_semantic::check(&ast);
    let checked = checked.unwrap_or_else(|| panic!("{diags:#?}"));
    let module = brim_lower::lower(&checked);
    brim_ir::validate(&module).expect("valid IR");
    module
}

fn run_with(src: &str, limits: Limits) -> (Result<i32, Trap>, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let mut host = Host {
        out: &mut out,
        err: &mut err,
        input: &mut input,
        args: Vec::new(),
        cwd: std::path::PathBuf::from("."),
    };
    let result = run(&compile(src), &mut host, limits);
    (result, String::from_utf8(out).unwrap())
}

fn output(src: &str) -> String {
    let (result, out) = run_with(src, Limits::default());
    result.expect("program runs");
    out
}

fn run_trap(src: &str) -> (Trap, String) {
    let (result, out) = run_with(src, Limits::default());
    (result.expect_err("program traps"), out)
}

#[test]
fn hello_world() {
    assert_eq!(
        output(
            r#"
fn main() {
    let name: string = "Ordnary"
    let version: string = "0.1.0"

    print("Hello from " + name)
    print("Brim version: " + version)
}
"#
        ),
        "Hello from Ordnary\nBrim version: 0.1.0\n"
    );
}

#[test]
fn functions_and_arithmetic() {
    assert_eq!(
        output(
            "fn add(a: i64, b: i64) -> i64 {\n return a + b\n}\nfn main() {\n let result: i64 = add(10, 32)\n print(result)\n print(7 / 2)\n print(-7 % 3)\n print(2 + 3 * 4 - 10 / 5)\n}"
        ),
        "42\n3\n-1\n12\n"
    );
}

#[test]
fn booleans_comparisons_and_strings() {
    assert_eq!(
        output(
            r#"fn main() {
    let a: i64 = 5
    print(a > 3 && a <= 5)
    print(a == 4 || !(a != 5))
    print("abc" == "abc")
    print("abc" != "abd")
    print(true == false)
}"#
        ),
        "true\ntrue\ntrue\ntrue\nfalse\n"
    );
}

#[test]
fn short_circuit_skips_side_effects() {
    assert_eq!(
        output(
            r#"
fn noisy(label: string, value: bool) -> bool {
    print(label)
    return value
}
fn main() {
    print(noisy("a", false) && noisy("b", true))
    print(noisy("c", true) || noisy("d", true))
    print(noisy("e", true) && noisy("f", false))
}
"#
        ),
        "a\nfalse\nc\ntrue\ne\nf\nfalse\n"
    );
}

#[test]
fn nested_calls_and_recursion_depth() {
    // Recursion without control flow cannot terminate, so it must hit the
    // depth limit cleanly instead of overflowing the host stack.
    let (trap, out) = run_trap(
        "fn f(n: i64) -> i64 {\n    return f(n + 1)\n}\nfn main() {\n    print(1)\n    print(f(0))\n}",
    );
    assert_eq!(out, "1\n");
    assert_eq!(trap.error, RuntimeError::StackOverflow { limit: 10_000 });
    assert_eq!(trap.backtrace.len(), 10_000);
    assert_eq!(trap.backtrace[0], "main");
}

#[test]
fn overflow_reports_location() {
    let src = "fn main() {\n    let big: i64 = 9223372036854775807\n    print(\"before\")\n    print(big + 1)\n}";
    let (trap, out) = run_trap(src);
    assert_eq!(out, "before\n");
    assert_eq!(trap.error, RuntimeError::IntegerOverflow { op: "+" });
    let file = SourceFile::new("t.brim", src);
    let span = trap.span.unwrap();
    assert_eq!(file.slice(span), "big + 1");
    assert_eq!(file.location(span.start).to_string(), "4:11");
    assert_eq!(trap.backtrace, vec!["main"]);
}

#[test]
fn division_by_zero() {
    let (trap, _) = run_trap(
        "fn div(a: i64, b: i64) -> i64 {\n return a / b\n}\nfn main() {\n print(div(1, 0))\n}",
    );
    assert_eq!(trap.error, RuntimeError::DivisionByZero { op: "/" });
    assert_eq!(trap.backtrace, vec!["main", "div"]);
    let (trap, _) = run_trap("fn main() {\n print(-9223372036854775808 / -1)\n}");
    assert_eq!(trap.error, RuntimeError::IntegerOverflow { op: "/" });
}

#[test]
fn write_errors_are_reported() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk on fire"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut err_out = Vec::new();
    let mut input = std::io::empty();
    let mut host = Host {
        out: &mut Broken,
        err: &mut err_out,
        input: &mut input,
        args: Vec::new(),
        cwd: std::path::PathBuf::from("."),
    };
    let err = run(
        &compile("fn main() { print(1) }"),
        &mut host,
        Limits::default(),
    )
    .unwrap_err();
    assert_eq!(err.error.code(), "R0004");
}

#[test]
fn loops_and_conditionals() {
    assert_eq!(
        output(
            r#"
fn fib(n: i64) -> i64 {
    if n < 2 {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}

fn main() {
    let mut i: i64 = 1
    let mut line: string = ""
    while i <= 15 {
        if i % 15 == 0 {
            line += "FizzBuzz"
        } else if i % 3 == 0 {
            line += "Fizz"
        } else if i % 5 == 0 {
            line += "Buzz"
        } else {
            line += to_string(i)
        }
        line += " "
        i += 1
    }
    print(line)
    print(fib(20))
}
"#
        ),
        "1 2 Fizz 4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz \n6765\n"
    );
}

#[test]
fn break_continue_and_nested_loops() {
    assert_eq!(
        output(
            r#"
fn main() {
    let mut row: i64 = 0
    while true {
        row += 1
        if row > 3 {
            break
        }
        let mut col: i64 = 0
        let mut cells: string = ""
        while col < 5 {
            col += 1
            if col == 2 {
                continue
            }
            if col > row + 1 {
                break
            }
            cells += to_string(col)
        }
        print(to_string(row) + ": " + cells)
    }
    print("done")
}
"#
        ),
        "1: 1\n2: 13\n3: 134\ndone\n"
    );
}

#[test]
fn compound_assignment_traps_on_overflow() {
    let (trap, out) = run_trap(
        "fn main() {\n    let mut x: i64 = 1\n    while true {\n        x *= 10\n        print(x)\n    }\n}",
    );
    assert_eq!(out.lines().count(), 18);
    assert_eq!(trap.error, RuntimeError::IntegerOverflow { op: "*" });
}
