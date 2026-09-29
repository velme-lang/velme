//! Explain mode (`runtime/30` §9): waves in plain words, from the program alone.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use velme_runtime::explain;
use velme_test_support::{goal_id, program};

const SOURCE: &str = "language: velme/0.1

goal Base(n: Number) -> Number:
    call:
        inner = Plain(n)
    plan: \"Return the inner number.\"

goal Plain(n: Number) -> Number:
    call:
        result = Leaf(n)

goal Leaf(n: Number) -> Number:
    plan: \"Return n. Nothing else happens here.\"

goal Abbrev(n: Number) -> Number:
    plan: \"Use the player's avg. score, e.g. 820. Then stop.\"

goal Top(n: Number) -> Number:
    call:
        a = Base(n)
        b = Leaf(n)
        c = Plain(a)
    plan: \"Add a, b and c.\"
";

/// A wave of one binding is "First:" / "Then:", several are "At the same time:"; a callee's line is its name in words and
/// its plan's first sentence, or ``run `Child` `` when it has none; a wired goal ends with its answer (R-RUN-22).
#[test]
fn ac_run_10_explain_words_the_waves() {
    let program = program(SOURCE);
    let text = explain(&program, goal_id(&program, "Top")).expect("explained");
    assert_eq!(
        text,
        "Top\n\
         At the same time:\n  - Base — Return the inner number.\n  - Leaf — Return n.\n\
         Then: Plain — run `Plain`\n\
         Finally: Add a, b and c.\n"
    );
    let wired = explain(&program, goal_id(&program, "Plain")).expect("explained");
    assert_eq!(wired, "Plain\nFirst: Leaf — Return n.\nThe answer is `result`.\n");
    let leaf = explain(&program, goal_id(&program, "Leaf")).expect("explained");
    assert_eq!(leaf, "Leaf\nThis goal calls no others: Return n.\n");
}

/// An abbreviation doesn't end a plan's first sentence: only a stop before a capital letter or the end does.
#[test]
fn a_plans_first_sentence_survives_abbreviations() {
    let program = program(SOURCE);
    let text = explain(&program, goal_id(&program, "Abbrev")).expect("explained");
    assert_eq!(
        text,
        "Abbrev\nThis goal calls no others: Use the player's avg. score, e.g. 820.\n"
    );
}
