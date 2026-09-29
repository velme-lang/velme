//! Parser: the golden corpus (R-SYN-20, D-70) and the `language/10` criteria it owns.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use velme_diagnostics::render::{self, JsonDiagnostic, LineIndex};
use velme_diagnostics::{Code, Diagnostic};
use velme_syntax::ast::{Decl, Expr, ExprKind, GoalDecl, Program};
use velme_syntax::{Keyword, SourceFile, parse};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parse_str(text: &str) -> (Program, Vec<Diagnostic>) {
    parse(&SourceFile::new("test.velme", text))
}

fn parse_path(path: &Path) -> (Program, Vec<Diagnostic>) {
    let bytes = std::fs::read(path).expect("golden file is readable");
    let file = SourceFile::from_bytes(path.display().to_string(), bytes).expect("golden file is UTF-8");
    parse(&file)
}

fn diagnostics_json(diags: &[Diagnostic]) -> Value {
    diags
        .iter()
        .map(|d| {
            json!({
                "code": d.code,
                "severity": d.severity,
                "message": d.message,
                "span": d.span,
                "help": d.help,
            })
        })
        .collect()
}

fn codes(diags: &[Diagnostic]) -> Vec<Code> {
    diags.iter().map(|d| d.code).collect()
}

/// The AST as JSON with every `span` removed, to compare shapes across layouts.
fn without_spans(program: &Program) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.remove("span");
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(program).expect("the AST serializes");
    strip(&mut v);
    v
}

fn only_goal(program: &Program) -> &GoalDecl {
    match program.decls.as_slice() {
        [Decl::Goal(goal)] => goal,
        other => panic!("expected one goal, got {other:#?}"),
    }
}

/// Parses `src` as the only check of a goal and returns it.
fn check_expr(src: &str) -> Expr {
    let text = format!("goal G(a: Boolean) -> Boolean:\n    plan: \"p\"\n    check:\n        - {src}\n");
    let (program, diags) = parse_str(&text);
    assert!(diags.is_empty(), "{src}: {diags:#?}");
    let check = only_goal(&program).check.as_ref().expect("a check block");
    check.items.first().expect("one check item").clone()
}

/// A compact rendering of an expression's tree shape.
fn shape(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Number { text } => text.clone(),
        ExprKind::Text { value } => format!("{value:?}"),
        ExprKind::Bool { value } => value.to_string(),
        ExprKind::Nothing => "nothing".into(),
        ExprKind::Result => "result".into(),
        ExprKind::Name { name } => name.clone(),
        ExprKind::Call { callee, args } => {
            let args: Vec<String> = args
                .iter()
                .map(|a| match &a.name {
                    Some(n) => format!("{}: {}", n.name, shape(&a.value)),
                    None => shape(&a.value),
                })
                .collect();
            format!("{}({})", callee.name, args.join(", "))
        }
        ExprKind::List { items } => format!("[{}]", items.iter().map(shape).collect::<Vec<_>>().join(", ")),
        ExprKind::Field { base, field } => format!("{}.{}", shape(base), field.name),
        ExprKind::Unary { op, operand } => format!("({op:?} {})", shape(operand)),
        ExprKind::Binary { op, lhs, rhs } => format!("({op:?} {} {})", shape(lhs), shape(rhs)),
        ExprKind::IsEmpty { operand, negated } => {
            format!("(IsEmpty{} {})", if *negated { "Not" } else { "" }, shape(operand))
        }
        ExprKind::If { condition, then } => format!("(If {} {})", shape(condition), shape(then)),
        ExprKind::Quantified {
            quantifier,
            var,
            collection,
            body,
        } => format!("({quantifier:?} {} {} {})", var.name, shape(collection), shape(body)),
    }
}

// ---- golden corpus ----

#[test]
fn golden_accept() {
    insta::glob!("../../../tests/golden/parser/accept", "*.velme", |path| {
        let (program, diags) = parse_path(path);
        assert!(diags.is_empty(), "{}: {diags:#?}", path.display());
        insta::assert_json_snapshot!(program);
    });
}

#[test]
fn golden_reject() {
    insta::glob!("../../../tests/golden/parser/reject", "*.velme", |path| {
        let (_, diags) = parse_path(path);
        assert!(!diags.is_empty(), "{} produced no diagnostics", path.display());
        // Rendered by serde_json, not insta's serializer: with `arbitrary_precision` (unified in from `velme-ir`)
        // insta would print each number as serde_json's private token map.
        insta::assert_snapshot!(
            serde_json::to_string_pretty(&diagnostics_json(&diags)).expect("JSON value serializes")
        );
    });
}

/// R-SYN-20: every EBNF production in `language/10` §4 is named by a `# covers:` line in an accepting and a
/// rejecting golden file.
#[test]
fn golden_corpus_covers_every_production() {
    let spec = std::fs::read_to_string(repo_root().join("docs/spec/language/10-syntax-grammar.md"))
        .expect("language/10 is readable");
    let grammar = spec
        .split("## 4. Grammar")
        .nth(1)
        .and_then(|s| s.split("```text").nth(1))
        .and_then(|s| s.split("```").next())
        .expect("language/10 §4 has an EBNF block");
    let productions: BTreeSet<&str> = grammar
        .lines()
        .filter_map(|l| l.split_once(" =").map(|(lhs, _)| lhs.trim()))
        .filter(|lhs| !lhs.is_empty() && lhs.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .collect();
    assert!(productions.len() > 30, "found only {productions:?}");
    for dir in ["accept", "reject"] {
        let mut covered = BTreeSet::new();
        for entry in std::fs::read_dir(repo_root().join("tests/golden/parser").join(dir)).expect("corpus dir") {
            let text = std::fs::read_to_string(entry.expect("dir entry").path()).expect("golden file");
            for line in text.lines().filter_map(|l| l.strip_prefix("# covers:")) {
                covered.extend(line.split(',').map(|s| s.trim().to_owned()));
            }
        }
        let missing: Vec<&&str> = productions.iter().filter(|p| !covered.contains(**p)).collect();
        assert!(missing.is_empty(), "{dir}/ lacks a file covering {missing:?}");
        let unknown: Vec<&String> = covered.iter().filter(|c| !productions.contains(c.as_str())).collect();
        assert!(unknown.is_empty(), "{dir}/ names productions not in §4: {unknown:?}");
    }
}

// ---- acceptance criteria ----

#[test]
fn ac_syn_01_complete_program_parses_cleanly() {
    let spec = std::fs::read_to_string(repo_root().join("docs/spec/language/12-goals-calls.md"))
        .expect("language/12 is readable");
    let program = spec
        .split("### 8.1 Complete program")
        .nth(1)
        .and_then(|s| s.split("```text\n").nth(1))
        .and_then(|s| s.split("```").next())
        .expect("language/12 §8.1 has a program");
    let golden =
        std::fs::read_to_string(repo_root().join("tests/golden/parser/accept/spec_8_1_complete_program.velme"))
            .expect("golden file");
    let golden_body: String = golden
        .lines()
        .skip_while(|l| l.starts_with('#'))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(golden_body, program, "the golden file is the §8.1 program");
    let (ast, diags) = parse_str(program);
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(ast.decls.len(), 5);
}

#[test]
fn ac_syn_02_tab_does_not_stop_parsing() {
    let src = "type A:\n\tx: Number\n\ntype B:\n    y Number\n\ntype C:\n    z: Number\n";
    let (program, diags) = parse_str(src);
    assert_eq!(codes(&diags), [Code::TabIndentation, Code::UnexpectedToken]);
    // A declaration holding any error is excluded, the tab's included (D-76).
    assert_eq!(decl_names(&program), ["C"]);
    assert_eq!(program.failed.len(), 2);
}

fn decl_names(program: &Program) -> Vec<&str> {
    program
        .decls
        .iter()
        .map(|d| match d {
            Decl::Type(t) => t.name.name.as_str(),
            Decl::Goal(g) => g.name.name.as_str(),
        })
        .collect()
}

/// D-76: one mistake is one diagnostic, and the declaration holding it is excluded.
#[test]
fn ac_syn_07_one_error_per_root_cause() {
    let one = |src: &str, code: Code| {
        let (program, diags) = parse_str(src);
        assert_eq!(codes(&diags), [code], "{src}: {diags:#?}");
        assert!(program.decls.is_empty(), "{src}");
        assert_eq!(program.failed.len(), 1, "{src}");
    };
    one("type Résumé:\n    x: Number\n", Code::UnexpectedToken);
    one(
        "goal A() -> Text:\n    when:\n        - true\n    plan: \"x\"\n",
        Code::ReservedWord,
    );
    one("goal A() -> Text:\n    plan: @\n", Code::UnexpectedToken);
    one("goal A() -> Text:\n    plan: \"x\" @@@\n", Code::UnexpectedToken);
    one("goal A() -> Text:\n    plan: \"a\\qb\"\n", Code::UnexpectedToken);
    // A line between two indentation levels stays in its block, so `check:` isn't lost.
    let src = "goal A() -> Text:\n        plan: \"x\"\n    check:\n        - result is not empty\n";
    one(src, Code::InconsistentIndentation);
    // One line off in a block: the lines after it at the block's width are still its siblings.
    one(
        "type A:\n    x: Number\n  y: Number\n    z: Number\n",
        Code::InconsistentIndentation,
    );
    // Old line endings are one error, yet a real mistake after them is still reported.
    let (_, diags) = parse_str("type A:\r    x: Number\r\rtype B:\r    y Number\r");
    assert_eq!(
        codes(&diags),
        [Code::UnexpectedToken, Code::UnexpectedToken],
        "{diags:#?}"
    );
    // D-73: a plan line shallower than the first is VL0102, and the plan doesn't swallow it silently.
    one(
        "goal A() -> Text:\n    plan: |\n        Hi.\n      check:\n        - true\n",
        Code::InconsistentIndentation,
    );
}

#[test]
fn ac_syn_05_reserved_parameter_name() {
    let (_, diags) = parse_str("goal A(when: Number) -> Number:\n    plan: \"p\"\n");
    assert_eq!(codes(&diags), [Code::ReservedWord]);
    let message = &diags.first().expect("one diagnostic").message;
    assert!(
        message.contains("`when`") && message.contains("later Velme version"),
        "{message}"
    );
}

#[test]
fn ac_syn_07_three_errors_in_source_order() {
    let src = "type A:\n    x Number\n\ngoal B() -> Number:\n    plan: \"p\"\n    check:\n        - result >\n\n\
               goal C() -> Number\n    plan: \"p\"\n";
    let (program, diags) = parse_str(src);
    assert_eq!(codes(&diags), [Code::UnexpectedToken; 3], "{diags:#?}");
    assert!(diags.windows(2).all(|w| w[0].span.start < w[1].span.start));
    assert!(!codes(&diags).contains(&Code::UnknownGoal));
    assert_eq!(program.failed.len(), 3);
}

#[test]
fn ac_syn_08_header_version() {
    let (_, diags) = parse_str("language: velme/0.2\n");
    assert_eq!(codes(&diags), [Code::UnsupportedLanguageVersion]);
    let (program, diags) = parse_str("type A:\n    x: Number\n");
    assert!(diags.is_empty() && program.header.is_none());
}

#[test]
fn ac_syn_09_comparison_and_boolean_precedence() {
    let (_, diags) = parse_str("goal G() -> Boolean:\n    check:\n        - a < b < c\n");
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert_eq!(shape(&check_expr("not a and b")), "(And (Not a) b)");
    assert_eq!(shape(&check_expr("a or b and c")), "(Or a (And b c))");
}

#[test]
fn ac_syn_10_if_and_quantifiers_extend_right() {
    assert_eq!(shape(&check_expr("if a then b or c")), "(If a (Or b c))");
    assert_eq!(
        shape(&check_expr("every x in xs has x > 0 and x < 9")),
        "(Every x xs (And (Gt x 0) (Lt x 9)))"
    );
}

#[test]
fn ac_syn_11_split_parameter_list() {
    let one = "goal G(a: Number, b: Text) -> Number:\n    plan: \"p\"\n";
    let split = "goal G(\n    a: Number,\n        b: Text\n) -> Number:\n    plan: \"p\"\n";
    let (one, d1) = parse_str(one);
    let (split, d2) = parse_str(split);
    assert!(d1.is_empty() && d2.is_empty(), "{d1:#?} {d2:#?}");
    assert_eq!(without_spans(&one), without_spans(&split));
}

#[test]
fn ac_syn_12_block_order_hint() {
    let (_, diags) = parse_str("goal G() -> Number:\n    check:\n        - result > 0\n    plan: \"p\"\n");
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    let d = diags.first().expect("one diagnostic");
    assert!(d.message.contains("`plan:`"), "{}", d.message);
    let help = d.help.as_deref().unwrap_or_default();
    assert!(
        help.contains("`budget`, `call:`, `plan:`, `check:`, `examples:`"),
        "{help}"
    );
}

/// R-SYN-19 and D-71: nesting past the limit is a diagnostic, never a stack overflow, and nesting up to it parses.
#[test]
fn ac_syn_13_deep_nesting() {
    let deep = |n: usize| format!("{}a{}", "(".repeat(n), ")".repeat(n));
    // Inside a goal and its `check:` block, 2 levels of indentation, the bullet and the line leave 28.
    let _ = check_expr(&deep(28));
    let _ = check_expr(&format!("{}a", "not ".repeat(28)));
    let _ = check_expr(&format!("{}1", "- ".repeat(28)));
    let src = format!("goal G() -> Boolean:\n    check:\n        - {}\n", deep(10_000));
    let (_, diags) = parse_str(&src);
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert!(diags.iter().any(|d| d.message.contains("nests too deeply")));

    // A staircase of ever-deeper lines: recovery recurses per indentation level.
    let mut stairs = String::from("type A:\n x: Number\n");
    for width in 2..2_000 {
        stairs.push_str(&format!("{}y\n", " ".repeat(width)));
    }
    let (_, diags) = parse_str(&stairs);
    assert!(
        diags.iter().any(|d| d.message.contains("nests too deeply")),
        "{:?}",
        diags.first()
    );

    // Past the limit, prefix forms and a type's `<` are rejected too.
    for src in [format!("{}a", "not ".repeat(40)), format!("{}1", "- ".repeat(40))] {
        let (_, diags) = parse_str(&format!("goal G() -> Boolean:\n    check:\n        - {src}\n"));
        assert!(diags.iter().any(|d| d.message.contains("nests too deeply")), "{src}");
    }
    let list = |n: usize| format!("{}Number{}", "List<".repeat(n), ">".repeat(n));
    let (_, diags) = parse_str(&format!("type A:\n    x: {}\n", list(29)));
    assert!(diags.is_empty(), "{diags:#?}");
    let (_, diags) = parse_str(&format!("type A:\n    x: {}\n", list(40)));
    assert!(diags.iter().any(|d| d.message.contains("nests too deeply")));
    // Flat forms don't nest: side by side, each ends before the next starts (D-71).
    let _ = check_expr(&vec!["a < 1"; 40].join(" and "));
    let _ = check_expr(&vec!["not a"; 40].join(" and "));
    let _ = check_expr(&format!("{} > 0", vec!["-a"; 40].join(" + ")));
    let _ = check_expr(&format!("{} > 0", vec!["(a)"; 40].join(" + ")));

    // A long operator chain builds a tree as deep as the chain.
    let _ = check_expr(&format!("{} > 0", vec!["a"; 250].join(" + ")));
    let chain = vec!["a"; 20_000].join(" + ");
    let (_, diags) = parse_str(&format!("goal G() -> Boolean:\n    check:\n        - {chain} > 0\n"));
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert!(diags.iter().any(|d| d.message.contains("too many operators")));
}

#[test]
fn ac_syn_13_fuzz_corpus_replays_without_panic() {
    let dirs = [
        "fuzz/corpus/parse",
        "tests/golden/parser/accept",
        "tests/golden/parser/reject",
        "tests/golden/lexer",
    ];
    let mut count = 0;
    for dir in dirs {
        for entry in std::fs::read_dir(repo_root().join(dir)).expect("corpus dir") {
            let path = entry.expect("dir entry").path();
            let bytes = std::fs::read(&path).expect("corpus file");
            if let Ok(file) = SourceFile::from_bytes(path.display().to_string(), bytes) {
                // Like the fuzz target: the renderers are total too.
                let (_, diags) = parse(&file);
                let _ = render::render_human(&diags, &file.path, Some(&file.text), false);
                let lines = LineIndex::new(&file.text);
                let _: Vec<_> = diags
                    .iter()
                    .map(|d| JsonDiagnostic::new(d, &file.path, &lines))
                    .collect();
            }
            count += 1;
        }
    }
    assert!(count > 50, "only {count} corpus files");
}

mod totality {
    use proptest::prelude::*;

    /// Pieces of Velme, so random input reaches deep into the grammar.
    const FRAGMENTS: &[&str] = &[
        "language: velme/0.1\n",
        "type ",
        "goal ",
        "plan",
        "check",
        "examples",
        "call",
        "budget",
        ":",
        "|",
        " ",
        "\n",
        "    ",
        "\t",
        "\r",
        "(",
        ")",
        "[",
        "]",
        "<",
        ">",
        ",",
        ".",
        "?",
        "-",
        "->",
        "=",
        "==",
        "<=",
        "+",
        "*",
        "/",
        "\"",
        "\"text\"",
        "#",
        "a",
        "Player",
        "List",
        "result",
        "if ",
        "then ",
        "every ",
        "some ",
        "in ",
        "has ",
        "not ",
        "and ",
        "or ",
        "is ",
        "empty",
        "nothing",
        "true",
        "1",
        "2.5",
        "1_0",
        "5s",
        "when",
        "\\u{41}",
    ];

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn ac_syn_13_parser_is_total_on_any_text(text in "\\PC{0,200}") {
            let _ = super::parse_str(&text);
        }

        #[test]
        fn ac_syn_13_parser_is_total_on_velme_fragments(
            parts in prop::collection::vec(prop::sample::select(FRAGMENTS), 0..120)
        ) {
            let _ = super::parse_str(&parts.concat());
        }
    }
}

#[test]
fn ac_syn_14_bom_then_source() {
    let (program, diags) = parse_str("\u{feff}type A:\n    x: Number\n");
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(program.decls.len(), 1);
}

#[test]
fn ac_syn_15_crlf_parses_like_lf() {
    let lf = "type A:\n    x: Number\n\ngoal G(a: A) -> Number:\n    plan: |\n        Add one.\n\n        Done.\n";
    let crlf = lf.replace('\n', "\r\n");
    let (a, d1) = parse_str(lf);
    let (b, d2) = parse_str(&crlf);
    assert!(d1.is_empty() && d2.is_empty(), "{d1:#?} {d2:#?}");
    assert_eq!(without_spans(&a), without_spans(&b));
}

#[test]
fn ac_syn_16_number_range() {
    let fraction_29 = format!("0.{}", "1".repeat(29));
    let fraction_28 = format!("0.{}", "1".repeat(28));
    let (_, diags) = parse_str(&format!(
        "goal G() -> Boolean:\n    check:\n        - {fraction_29} > 0\n"
    ));
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert!(diags.iter().any(|d| d.message.contains("too many decimal places")));
    let (_, diags) = parse_str("goal G() -> Boolean:\n    check:\n        - 79228162514264337593543950336 > 0\n");
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert!(diags.iter().any(|d| d.message.contains("too big")));
    let _ = check_expr(&format!("{fraction_28} > 0"));
    let _ = check_expr("79_228_162_514_264_337_593_543_950_335 > 0");
    let _ = check_expr("7922816251426433759354395033.5 > 0");
}

#[test]
fn ac_syn_17_double_optional_and_version_text() {
    let (_, diags) = parse_str("type A:\n    p: Player??\n");
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    let (_, diags) = parse_str("language: velme/0.10\n");
    assert_eq!(codes(&diags), [Code::UnsupportedLanguageVersion]);
    let (program, diags) = parse_str("language: velme/0.1\n");
    assert!(diags.is_empty() && program.header.is_some());
}

#[test]
fn ac_syn_18_calls_and_record_literals() {
    let (_, diags) = parse_str("goal G() -> Boolean:\n    check:\n        - result == Player(name: \"A\", 3)\n");
    assert_eq!(codes(&diags), [Code::UnexpectedToken]);
    assert_eq!(shape(&check_expr("sum(1, 2) > 0")), "(Gt sum(1, 2) 0)");
    assert_eq!(
        shape(&check_expr("result == Player(name: \"A\", jump_height: 3, score: 1)")),
        "(Eq result Player(name: \"A\", jump_height: 3, score: 1))"
    );
}

/// The parser half of AC-TYP-16; the JSON half is in `velme-ir`'s mapping tests.
#[test]
fn ac_typ_16_untyped_parameter_help_names_it() {
    for (params, found) in [("(count)", "`)`"), ("(count, b: Number)", "`,`")] {
        let (_, diags) = parse_str(&format!("goal A{params} -> Number:\n    plan: \"p\"\n"));
        let [diag] = diags.as_slice() else {
            panic!("one diagnostic for {params}: {diags:?}")
        };
        assert_eq!(diag.code, Code::UnexpectedToken);
        assert_eq!(
            diag.message,
            format!("I didn't expect {found} here — I was looking for `:`.")
        );
        assert_eq!(diag.help.as_deref(), Some("give `count` a type, like `count: Number`"));
    }
}

#[test]
fn ac_err_02_every_reserved_word_as_a_name() {
    for kw in Keyword::ALL.iter().filter(|k| k.is_reserved()) {
        let (_, diags) = parse_str(&format!("goal A({kw}: Number) -> Number:\n    plan: \"p\"\n"));
        assert_eq!(codes(&diags), [Code::ReservedWord], "{kw}");
        let (_, diags) = parse_str(&format!("type {kw}:\n    x: Number\n"));
        assert!(codes(&diags).contains(&Code::ReservedWord), "{kw}");
    }
}
