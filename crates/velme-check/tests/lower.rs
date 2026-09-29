//! Lowering of checks and examples to IR (`language/13` R-CHK-11).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
// The lists the fixture directories: tests may read files, the library may not (R-RUN-05).
#![allow(clippy::disallowed_methods)]

use serde_json::to_string_pretty;
use velme_check::{Lowered, lower_check, lower_example};
use velme_ir::CheckScope;
use velme_test_support::{program, read, repo};

const PROGRAM: &str = r#"language: velme/0.1

type Ball:
    bounce: Number

type Player:
    name: Text
    best: Ball?

goal Lowering(players: List<Player>, top: Player?, count: Number) -> List<Ball>:
    plan: "Every surface form a check lowers."
    check:
        - every ball in result has ball.bounce >= 5
        - some p in players has p.name == "Lina"
        - if players is empty then result is empty
        - players.name.length == count
        - top is not empty and top.best is not empty and top.best.bounce > 0
        - -count / 2 < 007.50
        - contains(result, Ball(bounce: 1)) or not (result == [])
    examples:
        - Lowering([Player(name: "Lina", best: nothing)], nothing, 1) == [Ball(bounce: 5)]
"#;

/// The source text of each node of `lowered`, in pre-order.
fn texts(lowered: &Lowered) -> String {
    assert_eq!(
        lowered.spans.len(),
        lowered.node.node().preorder().len(),
        "one span per node"
    );
    let texts: Vec<&str> = lowered.spans.iter().map(|s| &PROGRAM[s.start..s.end]).collect();
    texts.join(" | ")
}

/// Each check item and example of [`PROGRAM`], lowered: the R-CHK-11 table, narrowing read through `unwrap_or`
/// (R-IR-05), and each node's source text.
#[test]
fn checks_and_examples_lower_to_ir() {
    let program = program(PROGRAM);
    let goal = &program.goals[0];
    let scope = CheckScope::new(&program, goal).expect("scope");
    let mut out = String::new();
    for check in &goal.checks {
        let lowered = lower_check(&program, goal, &scope, check).expect("lowers");
        out.push_str(&to_string_pretty(lowered.node.node()).expect("JSON"));
        out.push_str(&format!("\n{}\n", texts(&lowered)));
    }
    for example in &goal.examples {
        let (args, expected) = lower_example(&program, goal, &scope, example).expect("lowers");
        let nodes: Vec<_> = args.iter().map(|a| a.node.node()).collect();
        out.push_str(&to_string_pretty(&(nodes, expected.node.node())).expect("JSON"));
        for lowered in args.iter().chain([&expected]) {
            out.push_str(&format!("\n{}", texts(lowered)));
        }
        out.push('\n');
    }
    insta::assert_snapshot!(out);
}

/// R-CHK-11: every check item and example of the accepted golden programs lowers to IR that passes the validator's
/// name and type stages (`compiler/21` §6 stages 3–4) in its goal's check scope, the only IR a back end evaluates
/// besides a validated goal (INV-1).
#[test]
fn lowered_checks_and_examples_pass_validation() {
    let mut files = vec![
        repo("tests/golden/ir/goals.velme"),
        repo("tests/golden/checks/double.velme"),
    ];
    let accept = repo("tests/golden/sema/accept");
    let mut listed: Vec<_> = std::fs::read_dir(&accept)
        .expect("golden directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "velme"))
        .collect();
    listed.sort();
    files.extend(listed);
    let mut lowered = 0;
    for text in files.iter().map(|path| read(path)).chain([PROGRAM.to_owned()]) {
        let program = program(&text);
        for goal in &program.goals {
            let scope = CheckScope::new(&program, goal).expect("scope");
            for check in &goal.checks {
                lower_check(&program, goal, &scope, check).unwrap_or_else(|d| panic!("{}: {d:?}", goal.name));
                lowered += 1;
            }
            for example in &goal.examples {
                lower_example(&program, goal, &scope, example).unwrap_or_else(|d| panic!("{}: {d:?}", goal.name));
                lowered += 1;
            }
        }
    }
    assert!(lowered > 20, "{lowered}");
}

/// Checked source can pass the limits `compiler/21` §7 puts on synthesized IR, so a lowered check isn't held to them
/// (the check scope applies stages 2–4 only): a 200-operator chain nests past depth 128, five quantifiers pass the
/// collection nesting of 4, and literals pass 1 000 items and 64 KiB.
#[test]
fn lowered_checks_are_not_held_to_the_limits_of_synthesized_ir() {
    let chain = vec!["x"; 200].join(" + ");
    let numbers: Vec<String> = (0..1200).map(|i| i.to_string()).collect();
    let text = format!(
        "language: velme/0.1\n\ngoal G(x: Number, xs: List<Number>) -> Number:\n    plan: \"p\"\n    check:\n        \
         - {chain} > 0\n        \
         - every a in xs has every b in xs has every c in xs has every d in xs has every e in xs has a == e\n        \
         - contains([{}], x)\n        - \"{}\" != \"\"\n",
        numbers.join(", "),
        "a".repeat(70 * 1024)
    );
    // The stack the CLI analyzes and runs on (`runtime/30` R-RUN-25): a debug build parses a 200-operator chain deeper
    // than a test thread's default stack.
    let lowered = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let program = program(&text);
            let goal = &program.goals[0];
            let scope = CheckScope::new(&program, goal).expect("scope");
            for check in &goal.checks {
                lower_check(&program, goal, &scope, check).expect("lowers");
            }
            goal.checks.len()
        })
        .expect("thread")
        .join()
        .expect("lowers");
    assert_eq!(lowered, 4);
}
