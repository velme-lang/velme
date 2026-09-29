//! Lowering of checks and examples to IR (`language/13` R-CHK-11).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use serde_json::to_string_pretty;
use velme_check::{Lowered, lower_check, lower_example};
use velme_test_support::program;

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
    assert_eq!(lowered.spans.len(), lowered.node.preorder().len(), "one span per node");
    let texts: Vec<&str> = lowered.spans.iter().map(|s| &PROGRAM[s.start..s.end]).collect();
    texts.join(" | ")
}

/// Each check item and example of [`PROGRAM`], lowered: the R-CHK-11 table, narrowing read through `unwrap_or`
/// (R-IR-05), and each node's source text.
#[test]
fn checks_and_examples_lower_to_ir() {
    let program = program(PROGRAM);
    let goal = &program.goals[0];
    let mut out = String::new();
    for check in &goal.checks {
        let lowered = lower_check(&program, goal, check).expect("lowers");
        out.push_str(&to_string_pretty(&lowered.node).expect("JSON"));
        out.push_str(&format!("\n{}\n", texts(&lowered)));
    }
    for example in &goal.examples {
        let (args, expected) = lower_example(&program, goal, example).expect("lowers");
        let nodes: Vec<_> = args.iter().map(|a| &a.node).collect();
        out.push_str(&to_string_pretty(&(nodes, &expected.node)).expect("JSON"));
        for lowered in args.iter().chain([&expected]) {
            out.push_str(&format!("\n{}", texts(lowered)));
        }
        out.push('\n');
    }
    insta::assert_snapshot!(out);
}
