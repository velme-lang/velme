//! Velme test support (`delivery/51`): helpers shared by the test suites of several crates. A dev-dependency only.
#![forbid(unsafe_code)]
// A helper here fails the test that called it, so it panics on bad input like a test body does (CC-ERR-01 covers
// non-test code only).
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use velme_ir::{CallNode, Goal, Origin, Request, ValidIr, from_json_str, signature, validate};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};

/// `path`, relative to the repository root.
pub fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

/// The text of the file at `path`.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The checked program of `text`, which must have no errors.
pub fn program(text: &str) -> Program {
    let (program, diags) = analyze(&SourceFile::new("test.velme", text));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

/// The goal called `name`.
pub fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(
        program
            .goals
            .iter()
            .position(|g| g.name == name)
            .unwrap_or_else(|| panic!("no goal {name}")),
    )
}

/// Hand-written IR for a goal of `program`, validated as a whole goal: its own `calls` stand in for the compiler's, so
/// each must carry its child's real signature (`compiler/21` R-IR-09).
pub fn valid_ir(program: &Program, ir: &str) -> ValidIr {
    let goal: Goal = from_json_str(ir).unwrap_or_else(|e| panic!("IR doesn't parse: {e}"));
    for CallNode::Call(call) in &goal.calls {
        let child = signature(program, goal_id(program, &call.goal)).expect("a checked goal has a signature");
        assert_eq!(
            call.goal_signature,
            child.to_string(),
            "the call `{}` has a stale signature",
            call.binding
        );
    }
    let request = Request {
        program,
        goal: goal_id(program, &goal.goal),
        calls: &goal.calls,
        origin: Origin::Complete,
    };
    validate(ir, &request).unwrap_or_else(|d| panic!("IR doesn't validate: {d:#?}"))
}
