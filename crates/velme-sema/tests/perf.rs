//! The `delivery/51` §6 targets of parsing and analysis on a 1 000-line program from the seeded generator (D-130,
//! D-131). The timed tests are ignored and run in release by `cargo xtask gate`; the generator's own test is not.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Duration;

use velme_sema::{SourceFile, analyze};
use velme_test_support::workload::{assert_release, best_of, goals, source};

/// The seed of the program the targets are measured on.
const SEED: u64 = 1;

/// Every program the generator makes checks without a diagnostic, has the size asked for, and is the same for the same
/// seed (D-131).
#[test]
fn generated_programs_check_cleanly_at_the_size_asked_for() {
    for seed in 0..8 {
        let text = source(seed, 1000);
        assert_eq!(text.lines().count(), 1000);
        assert_eq!(text, source(seed, 1000));
        let (program, diags) = analyze(&SourceFile::new("gen.velme", &text));
        assert!(diags.is_empty(), "seed {seed}: {diags:#?}");
        assert!(program.expect("a program").goals.len() > 50);
        let (program, diags) = analyze(&SourceFile::new("gen.velme", goals(seed, 100)));
        assert!(diags.is_empty(), "seed {seed}: {diags:#?}");
        assert_eq!(program.expect("a program").goals.len(), 100);
    }
}

/// `analyze` of the generated 1 000-line program takes under 50 ms, the best of 20 (AC-CMP-07).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_cmp_07_analyze_of_a_generated_1000_line_program_is_under_50_ms() {
    assert_release("ac_cmp_07");
    let file = SourceFile::new("gen.velme", source(SEED, 1000));
    let took = best_of(20, || assert!(analyze(&file).1.is_empty()));
    println!("analyze, 1 000 lines: {took:.2?}");
    assert!(took < Duration::from_millis(50), "{took:?}");
}

/// Parsing the generated 1 000-line program takes under 10 ms, the best of 20 (AC-QA-07, `delivery/51` §6).
#[test]
#[ignore = "a release-mode target: cargo xtask gate"]
fn ac_qa_07_parse_of_1000_lines_is_under_10_ms() {
    assert_release("ac_qa_07");
    let file = SourceFile::new("gen.velme", source(SEED, 1000));
    let took = best_of(20, || assert!(velme_syntax::parse(&file).1.is_empty()));
    println!("parse, 1 000 lines: {took:.2?}");
    assert!(took < Duration::from_millis(10), "{took:?}");
}
