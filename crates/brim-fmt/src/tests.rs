use super::*;

fn fmt(src: &str) -> String {
    format(src).unwrap_or_else(|d| panic!("{d:?}"))
}

#[test]
fn spaces_between_tokens() {
    assert_eq!(
        fmt("fn add( a:i64,b :i64 )->i64{return a+b*-a}\n"),
        "fn add(a: i64, b: i64) -> i64 { return a + b * -a }\n"
    );
    assert_eq!(
        fmt("fn main(){let xs:List[i64]=[ 1,2 ]\nlet m:Map[string,i64]=[:]\nprint(xs [0]-1)}\n"),
        "fn main() { let xs: List[i64] = [1, 2]\n    let m: Map[string, i64] = [:]\n    print(xs[0] - 1) }\n"
    );
    assert_eq!(
        fmt("import text . format.{ pad , Report }\nfn main(){}\n"),
        "import text.format.{pad, Report}\nfn main() {}\n"
    );
    assert_eq!(
        fmt(
            "fn main(){let p:Point=Point{x:1,y:-2}\nfor i in 0 .. 3{print(!true)}\nlet v:i64=parse(s) ?.len()}\n"
        ),
        "fn main() { let p: Point = Point { x: 1, y: -2 }\n    for i in 0..3 { print(!true) }\n    let v: i64 = parse(s)?.len() }\n"
    );
    assert_eq!(
        fmt("fn swap [A,B](p:Pair[A,B])->Pair[B,A]{return Pair{a:p.b,b:p.a}}\n"),
        "fn swap[A, B](p: Pair[A, B]) -> Pair[B, A] { return Pair { a: p.b, b: p.a } }\n"
    );
}

#[test]
fn indentation_and_blank_lines() {
    let src = "\n\nfn main() {\n\n\nlet x: i64 = 1\n      if x > 0 {\nprint(x)\n  } else {\n  print(0)\n\n}\n\n\n\n}\n\n\nfn other() {\n}\n\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let x: i64 = 1\n    if x > 0 {\n        print(x)\n    } else {\n        print(0)\n    }\n}\n\nfn other() {\n}\n"
    );
}

#[test]
fn brackets_spanning_lines() {
    let src = "fn main() {\nlet xs: List[i64] = [1,\n2,\n3]\nprint(add(\nxs[0],\n  xs[1]))\nmatch xs.len() {\n2 => print(\"two\")\n_ => {\nprint(\"other\")\n}\n}\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let xs: List[i64] = [1,\n        2,\n        3]\n    print(add(\n        xs[0],\n        xs[1]))\n    match xs.len() {\n        2 => print(\"two\")\n        _ => {\n            print(\"other\")\n        }\n    }\n}\n"
    );
}

#[test]
fn continuation_lines() {
    let src = "fn main() {\nlet total: i64 = 1 +\n2\nlet ok: bool = total > 0 &&\n    total < 10 ||\nfalse\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    let total: i64 = 1 +\n        2\n    let ok: bool = total > 0 &&\n        total < 10 ||\n        false\n}\n"
    );
}

#[test]
fn comments_are_kept() {
    let src = "// The program.\nfn main() {   // entry   \n// inside\n  print(1)// one\n     // before the end\n}\n// trailing";
    assert_eq!(
        fmt(src),
        "// The program.\nfn main() { // entry\n    // inside\n    print(1) // one\n    // before the end\n}\n// trailing\n"
    );
}

#[test]
fn literals_are_kept_as_written() {
    assert_eq!(
        fmt("fn main(){print(\"a\\tb\\n\" + \"//not a comment\")}\n"),
        "fn main() { print(\"a\\tb\\n\" + \"//not a comment\") }\n"
    );
}

#[test]
fn syntax_errors_are_not_formatted() {
    let diags = format("fn main( {\n").unwrap_err();
    assert!(!diags.is_empty());
}

#[test]
fn formatting_is_idempotent() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    for dir in ["examples", "tests/integration", "tests/modules"] {
        let mut stack = vec![root.join(dir)];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "brim") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let once = fmt(&src);
            assert_eq!(fmt(&once), once, "{}", path.display());
            if dir == "examples" {
                assert_eq!(once, src, "{} is not formatted", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked > 20);
}

#[test]
fn function_types_and_values() {
    assert_eq!(
        fmt(
            "fn apply(f: fn (i64) -> i64) -> fn (i64) -> i64 {\n    return fn (x: i64) -> i64 {\n        return f(x)\n    }\n}\n"
        ),
        "fn apply(f: fn(i64) -> i64) -> fn(i64) -> i64 {\n    return fn(x: i64) -> i64 {\n        return f(x)\n    }\n}\n"
    );
    assert_eq!(
        fmt("fn main() {\n    let s = if n < 0 { 1 } else { 2 }\n}\n"),
        "fn main() {\n    let s = if n < 0 { 1 } else { 2 }\n}\n"
    );
}

#[test]
fn closing_a_block_inside_call_arguments() {
    let src = "fn main() {\n    print(run(fn(x: i64) -> i64 {\n        return x\n    }, 10))\n}\n";
    assert_eq!(fmt(src), src);
}
