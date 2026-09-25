//! Verify gate tests (AC-QA-01).

use xtask::verify::{self, Step};
use xtask::workspace_root;

#[test]
fn ac_qa_01_verify_runs_every_gate_step() {
    let names: Vec<&str> = verify::steps(false).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(names, ["fmt", "clippy", "test", "doc", "deny", "layering", "ac-audit"]);
}

#[test]
fn ac_qa_01_quick_verify_skips_the_slow_steps() {
    let names: Vec<&str> = verify::steps(true).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(names, ["fmt", "clippy", "test"]);
}

#[test]
fn ac_qa_01_verify_reports_a_failing_step_and_still_runs_the_rest() {
    let cargo = xtask::cargo_program();
    let step = |name, arg: &str| Step {
        name,
        program: cargo.clone(),
        args: vec![arg.to_owned()],
        envs: Vec::new(),
    };
    let steps = [step("bad", "--no-such-flag"), step("good", "--version")];
    let failed = verify::run_steps(&workspace_root().expect("root"), &steps);
    assert_eq!(failed, ["bad"]);
}
