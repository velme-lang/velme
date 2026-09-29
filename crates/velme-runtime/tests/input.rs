//! Goal inputs from the command line (`tooling/40` §3.1, R-CLI-07, D-23).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Read;

use velme_diagnostics::{Code, Diagnostic};
use velme_ir::limits::MAX_DEPTH;
use velme_runtime::{MAX_INPUT_BYTES, decode_inputs, read_input};
use velme_sema::hir::Program;
use velme_test_support::{goal_id, program};

const SOURCE: &str = "language: velme/0.1\n\ngoal Add(a: Number, b: Number) -> Number:\n    plan: \"Add.\"\n";

fn add() -> Program {
    program(SOURCE)
}

fn decode(input: Option<&str>, args: &[(&str, &str)]) -> Vec<Diagnostic> {
    let program = add();
    let args: Vec<(String, String)> = args.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect();
    match decode_inputs(&program, goal_id(&program, "Add"), input, &args) {
        Ok(_) => Vec::new(),
        Err(diags) => diags,
    }
}

/// `depth` nested lists around `0`.
fn nested(depth: usize) -> String {
    format!("{}0{}", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn reading_stops_past_the_limit() {
    // An endless reader: the limit, not the end of the input, stops the read.
    let diag = read_input(std::io::repeat(b' '), "standard input").expect_err("too big");
    assert_eq!(diag.code, Code::InvalidInput);
    assert_eq!(diag.message, "The input is too big.");
    let exact = std::io::repeat(b' ').take(MAX_INPUT_BYTES);
    assert_eq!(
        read_input(exact, "input.json").expect("at the limit").len() as u64,
        MAX_INPUT_BYTES
    );
    let diag = read_input(&b"{\"a\": \xff}"[..], "input.json").expect_err("not UTF-8");
    assert_eq!(diag.code, Code::InvalidInput);
    assert_eq!(diag.notes, ["it isn't UTF-8 text"]);
    assert_eq!(diag.span, Default::default());
}

/// One diagnostic per root cause: a value that couldn't be read isn't also reported missing (CC-ERR-04).
#[test]
fn an_unreadable_value_is_not_also_missing() {
    let diags = decode(None, &[("a", "oops"), ("b", "1")]);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].message, "Input `a` isn't valid JSON.");
    for input in ["{\"a\": ", "[1, 2]"] {
        let diags = decode(Some(input), &[]);
        assert_eq!(diags.len(), 1, "{input}: {diags:#?}");
    }
    // A value given by `--arg` still counts when `--input` couldn't be read.
    let diags = decode(Some("[1, 2]"), &[("a", "\"x\"")]);
    assert_eq!(diags.len(), 2, "{diags:#?}");
}

/// The input depth limit counts each value, whether it comes by `--arg` or inside the `--input` object.
#[test]
fn values_nest_at_most_the_limit_either_way() {
    let deep = |diags: &[Diagnostic]| diags.iter().any(|d| d.notes.iter().any(|n| n.contains("nests deeper")));
    for (depth, too_deep) in [(MAX_DEPTH, false), (MAX_DEPTH + 1, true)] {
        let value = nested(depth);
        let by_arg = decode(None, &[("a", &value), ("b", "1")]);
        let by_input = decode(Some(&format!("{{\"a\": {value}, \"b\": 1}}")), &[]);
        for diags in [by_arg, by_input] {
            // At the limit the value is read, and is only the wrong type.
            assert_eq!(deep(&diags), too_deep, "{depth}: {diags:#?}");
            assert_eq!(diags.len(), 1, "{depth}: {diags:#?}");
        }
    }
}
