use super::*;
use brim_diagnostics::SourceFile;

fn parse_ok(src: &str) -> Program {
    let (program, diags) = parse(src);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
    program
}

/// Parses `src` as the body of `main` and returns its statements.
fn body(src: &str) -> Vec<Stmt> {
    let program = parse_ok(&format!("fn main() {{\n{src}\n}}"));
    program.functions.into_iter().next().unwrap().body.stmts
}

/// Parses a single expression and renders it fully parenthesised.
fn expr(src: &str) -> String {
    match &body(src)[..] {
        [
            Stmt {
                kind: StmtKind::Expr(e),
                ..
            },
        ] => e.to_string(),
        other => panic!("expected one expression statement, got {other:#?}"),
    }
}

/// Returns `(code, message)` for every diagnostic.
fn errors(src: &str) -> Vec<(&'static str, String)> {
    let (_, diags) = parse(src);
    assert!(!diags.is_empty(), "expected diagnostics for {src:?}");
    diags
        .into_iter()
        .map(|d| (d.code.map_or("", |c| c.0), d.message))
        .collect()
}

fn codes(src: &str) -> Vec<&'static str> {
    errors(src).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn parses_hello_world() {
    let program = parse_ok(
        r#"
fn main() {
    let name: string = "Ordnary"
    print("Hello from " + name)
}
"#,
    );
    assert_eq!(program.functions.len(), 1);
    let main = &program.functions[0];
    assert_eq!(main.name.name, "main");
    assert!(main.params.is_empty());
    assert!(main.ret.is_none());
    assert_eq!(main.body.stmts.len(), 2);
    let StmtKind::Let {
        mutable,
        name,
        ty,
        init,
    } = &main.body.stmts[0].kind
    else {
        panic!("expected let");
    };
    assert!(!mutable);
    assert_eq!(name.name, "name");
    assert_eq!(ty.name.name, "string");
    assert_eq!(init.to_string(), "\"Ordnary\"");
    let StmtKind::Expr(call) = &main.body.stmts[1].kind else {
        panic!("expected expression statement");
    };
    assert_eq!(call.to_string(), "print((\"Hello from \" + name))");
}

#[test]
fn parses_parameters_and_return_type() {
    let program = parse_ok("fn add(a: i64, b: i64) -> i64 {\n    return a + b\n}");
    let add = &program.functions[0];
    let params: Vec<_> = add
        .params
        .iter()
        .map(|p| (p.name.name.as_str(), p.ty.name.name.as_str()))
        .collect();
    assert_eq!(params, vec![("a", "i64"), ("b", "i64")]);
    assert_eq!(add.ret.as_ref().unwrap().name.name, "i64");
    let StmtKind::Return(Some(value)) = &add.body.stmts[0].kind else {
        panic!("expected return with value");
    };
    assert_eq!(value.to_string(), "(a + b)");
}

#[test]
fn multiline_parameter_lists_and_trailing_commas() {
    let program = parse_ok("fn f(\n    a: i64,\n    b: bool,\n) -> bool\n{\n    return b\n}");
    assert_eq!(program.functions[0].params.len(), 2);
    assert_eq!(expr("f(\n  1,\n  2,\n)"), "f(1, 2)");
}

#[test]
fn operator_precedence() {
    assert_eq!(expr("1 + 2 * 3"), "(1 + (2 * 3))");
    assert_eq!(expr("1 * 2 + 3"), "((1 * 2) + 3)");
    assert_eq!(expr("1 - 2 - 3"), "((1 - 2) - 3)");
    assert_eq!(expr("8 / 4 / 2 % 3"), "(((8 / 4) / 2) % 3)");
    assert_eq!(expr("(1 + 2) * 3"), "((1 + 2) * 3)");
    assert_eq!(expr("-a * b"), "((-a) * b)");
    assert_eq!(expr("--a"), "(-(-a))");
    assert_eq!(expr("!a && b || c"), "(((!a) && b) || c)");
    assert_eq!(expr("a || b && c"), "(a || (b && c))");
    assert_eq!(expr("a + 1 < b * 2"), "((a + 1) < (b * 2))");
    assert_eq!(expr("a < b == c > d"), "((a < b) == (c > d))");
    assert_eq!(expr("a == b && c != d"), "((a == b) && (c != d))");
    assert_eq!(expr("f(1 + 2, g(x)) * 2"), "(f((1 + 2), g(x)) * 2)");
}

#[test]
fn newline_handling() {
    // A trailing operator continues the expression on the next line.
    assert_eq!(expr("1 +\n  2 *\n  3"), "(1 + (2 * 3))");
    // Inside parentheses newlines are ignored entirely.
    assert_eq!(expr("(1\n + 2)"), "(1 + 2)");
    // Semicolons separate statements on one line.
    assert_eq!(body("f(); g()\n\n\nh();").len(), 3);
    // `let` initialisers may start on the next line.
    assert_eq!(body("let x: i64 =\n    5").len(), 1);
}

#[test]
fn return_without_value() {
    let stmts = body("return");
    assert!(matches!(stmts[0].kind, StmtKind::Return(None)));
    let program = parse_ok("fn f() { return }");
    assert!(matches!(
        program.functions[0].body.stmts[0].kind,
        StmtKind::Return(None)
    ));
}

#[test]
fn nested_blocks() {
    let stmts = body("{\n let x: i64 = 1\n {\n }\n}");
    let StmtKind::Block(block) = &stmts[0].kind else {
        panic!("expected block");
    };
    assert_eq!(block.stmts.len(), 2);
}

#[test]
fn comments_are_ignored() {
    let program = parse_ok("// leading\nfn main() { // trailing\n    // inside\n}\n// end");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn spans_point_at_source() {
    let src = "fn main() {\n    let total: i64 = 1 + 22\n}";
    let file = SourceFile::new("t.brim", src);
    let stmts = body("    let total: i64 = 1 + 22");
    let StmtKind::Let { name, init, .. } = &stmts[0].kind else {
        panic!()
    };
    let (program, _) = parse(src);
    let StmtKind::Let {
        name: n2, init: i2, ..
    } = &program.functions[0].body.stmts[0].kind
    else {
        panic!()
    };
    assert_eq!(file.slice(n2.span), "total");
    assert_eq!(file.slice(i2.span), "1 + 22");
    assert_eq!(file.location(i2.span.start).to_string(), "2:22");
    assert_eq!(name.name, n2.name);
    assert_eq!(init.to_string(), "(1 + 22)");
    assert_eq!(
        file.slice(program.functions[0].span),
        src,
        "function span covers the whole declaration"
    );
}

#[test]
fn error_missing_paren() {
    assert_eq!(
        errors("fn main() {\n    print(\"hi\"\n}"),
        vec![("E0100", "expected `,` or `)`, found `}`".to_string())]
    );
}

#[test]
fn error_unclosed_block() {
    let (_, diags) = parse("fn main() {\n    print(1)\n");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "expected `}`, found end of file");
    assert_eq!(diags[0].secondary[0].message, "this block is never closed");
}

#[test]
fn error_missing_type_annotation() {
    assert_eq!(
        errors("fn main() {\n    let x = 5\n}"),
        vec![("E0102", "variable `x` needs a type annotation".to_string())]
    );
    assert_eq!(codes("fn f(a) {}"), vec!["E0102"]);
}

#[test]
fn error_reserved_keywords() {
    assert_eq!(
        errors("fn main() {\n    for x {\n        print(1)\n    }\n}"),
        vec![("E0101", "`for` is not supported yet".to_string())]
    );
    assert_eq!(codes("struct Point {}"), vec!["E0101"]);
    assert_eq!(codes("fn match() {}"), vec!["E0101"]);
    // Keywords cannot be used as names either.
    assert_eq!(
        errors("fn while() {}"),
        vec![(
            "E0100",
            "expected a function name, found `while`".to_string()
        )]
    );
}

#[test]
fn error_top_level_items() {
    assert_eq!(codes("let x: i64 = 1"), vec!["E0103"]);
    assert_eq!(codes("print(1)\nfn main() {}"), vec!["E0103"]);
}

#[test]
fn error_assignment() {
    assert_eq!(
        errors("fn main() {\n    f() = 2\n}"),
        vec![("E0104", "invalid assignment target".to_string())]
    );
    assert_eq!(codes("fn main() { 1 += 2 }"), vec!["E0104"]);
}

/// Returns the single statement in `main`'s body.
fn stmt(src: &str) -> StmtKind {
    let mut stmts = body(src);
    assert_eq!(stmts.len(), 1, "{stmts:#?}");
    stmts.remove(0).kind
}

#[test]
fn parses_let_mut_and_assignment() {
    let StmtKind::Let { mutable, .. } = stmt("let mut x: i64 = 1") else {
        panic!()
    };
    assert!(mutable);
    let StmtKind::Assign {
        target, op, value, ..
    } = stmt("x = x * 2")
    else {
        panic!()
    };
    assert_eq!((target.name.as_str(), op), ("x", None));
    assert_eq!(value.to_string(), "(x * 2)");
    for (src, expected) in [
        ("x += 1", BinaryOp::Add),
        ("x -= 1", BinaryOp::Sub),
        ("x *= 1", BinaryOp::Mul),
        ("x /= 1", BinaryOp::Div),
        ("x %= 1", BinaryOp::Rem),
    ] {
        let StmtKind::Assign { op, .. } = stmt(src) else {
            panic!("{src}")
        };
        assert_eq!(op, Some(expected), "{src}");
    }
    // The value may continue on the next line.
    assert!(matches!(
        stmt(
            "x =
  1 + 2"
        ),
        StmtKind::Assign { .. }
    ));
}

#[test]
fn parses_if_else_chains() {
    let StmtKind::If {
        cond,
        then_block,
        else_branch,
    } = stmt("if a < b {\n print(1)\n} else if a == b {\n print(2)\n} else {\n print(3)\n}")
    else {
        panic!()
    };
    assert_eq!(cond.to_string(), "(a < b)");
    assert_eq!(then_block.stmts.len(), 1);
    let StmtKind::If {
        cond, else_branch, ..
    } = else_branch.unwrap().kind
    else {
        panic!("expected else if")
    };
    assert_eq!(cond.to_string(), "(a == b)");
    assert!(matches!(
        else_branch.unwrap().kind,
        StmtKind::Block(Block { ref stmts, .. }) if stmts.len() == 1
    ));

    // `else` may start on the next line; an `if` without `else` is fine.
    assert!(matches!(
        stmt("if x {\n}\nelse {\n}"),
        StmtKind::If {
            else_branch: Some(_),
            ..
        }
    ));
    assert_eq!(body("if x {}\nprint(1)").len(), 2);
}

#[test]
fn parses_while_break_continue() {
    let StmtKind::While { cond, body: block } =
        stmt("while i < 10 {\n  if done { break }\n  continue\n}")
    else {
        panic!()
    };
    assert_eq!(cond.to_string(), "(i < 10)");
    assert!(matches!(block.stmts[1].kind, StmtKind::Continue));
    let StmtKind::If { then_block, .. } = &block.stmts[0].kind else {
        panic!()
    };
    assert!(matches!(then_block.stmts[0].kind, StmtKind::Break));
}

#[test]
fn control_flow_errors() {
    // Assignment instead of comparison in a condition.
    let (_, diags) = parse("fn main() {\n    if x = 1 {\n    }\n}");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].help, vec!["use `==` to compare two values"]);
    // Braces are required.
    assert_eq!(
        errors("fn main() {\n    while x print(1)\n}"),
        vec![(
            "E0100",
            "expected `{` after the `while` condition, found identifier `print`".to_string()
        )]
    );
    assert_eq!(
        errors("fn main() {\n    else {}\n}"),
        vec![("E0100", "`else` without a preceding `if`".to_string())]
    );
    assert_eq!(codes("fn main() { if x {} else print(1) }"), vec!["E0100"]);
}

#[test]
fn deep_block_nesting_is_rejected_without_overflow() {
    let deep = format!(
        "fn main() {{ {}{} }}",
        "{".repeat(10_000),
        "}".repeat(10_000)
    );
    assert!(codes(&deep).contains(&"E0105"));
    let chain = format!(
        "fn main() {{ if a {{}}{} }}",
        " else if a {}".repeat(10_000)
    );
    assert!(codes(&chain).contains(&"E0105"));
}

#[test]
fn error_chained_comparison() {
    assert_eq!(codes("fn main() { a < b < c }"), vec!["E0106"]);
    assert_eq!(codes("fn main() { a == b != c }"), vec!["E0106"]);
    // Parenthesised and mixed-precedence comparisons are fine.
    assert_eq!(expr("(a < b) == (b < c)"), "((a < b) == (b < c))");
}

#[test]
fn error_invalid_call_target() {
    assert_eq!(codes("fn main() { f()(1) }"), vec!["E0107"]);
    assert_eq!(codes("fn main() { 1(2) }"), vec!["E0107"]);
    // A parenthesised name is still a name.
    assert_eq!(expr("(f)(1)"), "f(1)");
}

#[test]
fn error_two_expressions_on_one_line() {
    assert_eq!(
        errors("fn main() {\n    let x: i64 = 1 2\n}"),
        vec![(
            "E0100",
            "expected a newline or `;` after the statement, found integer literal".to_string()
        )]
    );
}

#[test]
fn error_leading_operator_has_hint() {
    let (_, diags) = parse("fn main() {\n    let x: i64 = 1\n        + 2\n}");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "expected an expression, found `+`");
    assert!(diags[0].notes[0].contains("end the previous line with the operator"));
}

#[test]
fn recovers_and_reports_multiple_errors() {
    let src = "fn main() {\n    let = 1\n    print(\n}\nfn other( {\n}\nfn ok() {}\n";
    let (program, diags) = parse(src);
    assert_eq!(codes(src), vec!["E0100", "E0100", "E0100"]);
    assert_eq!(diags.len(), 3);
    // The valid function after the errors is still parsed.
    assert!(program.functions.iter().any(|f| f.name.name == "ok"));
}

#[test]
fn nested_function_reports_unclosed_block() {
    let (program, diags) = parse("fn main() {\n    print(1)\nfn helper() {}\n");
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].notes,
        vec!["functions cannot be declared inside other functions"]
    );
    assert_eq!(program.functions.len(), 2);
}

#[test]
fn deep_nesting_is_rejected_without_overflow() {
    let deep_parens = format!(
        "fn main() {{ {}1{} }}",
        "(".repeat(10_000),
        ")".repeat(10_000)
    );
    assert!(codes(&deep_parens).contains(&"E0105"));
    let deep_unary = format!("fn main() {{ {}1 }}", "-".repeat(10_000));
    assert!(codes(&deep_unary).contains(&"E0105"));
    let long_chain = format!("fn main() {{ 1{} }}", " + 1".repeat(10_000));
    assert!(codes(&long_chain).contains(&"E0105"));
    // Moderately long chains are fine.
    let ok_chain = format!("fn main() {{ 1{} }}", " + 1".repeat(MAX_NESTING / 2));
    parse_ok(&ok_chain);
}

#[test]
fn lexer_errors_are_included() {
    // The unterminated string swallows the rest of the line, so the call and
    // the block are reported as unclosed too.
    assert_eq!(
        codes("fn main() { print(\"oops) }"),
        vec!["E0002", "E0100", "E0100"]
    );
}

#[test]
fn invalid_characters_are_reported_once() {
    assert_eq!(
        codes("fn main() {\n    let x: i64 = 5 $ 3\n}"),
        vec!["E0001"]
    );
    assert_eq!(codes("@\nfn main() {}"), vec!["E0001"]);
    assert_eq!(codes("fn main() { print(@) }"), vec!["E0001"]);
}

#[test]
fn garbage_input_does_not_panic() {
    for src in [
        "",
        "}",
        "{{{{",
        "fn",
        "fn (",
        "fn main(",
        "fn main() ->",
        "fn main() -> {",
        "fn main() { let }",
        "fn main() { let x: }",
        "fn main() { return return }",
        ")))(((",
        "fn main() { ,,, }",
        "\"",
        "fn main() { f(,) }",
        "fn f(a: i64 b: i64) {}",
    ] {
        let _ = parse(src);
    }
}
