//! The IR validator is total: any bytes end in validated IR or diagnostics, never a panic (`compiler/21` R-IR-18,
//! AC-IR-06). The WASM emitter is too on whatever the validator accepts: a module that validates, a decline or a
//! composite goal, never a panic or a backend bug (R-SBX-01, R-SBX-16). Run with `cargo +nightly fuzz run validate`;
//! `fuzz/corpus/validate/` is replayed by `cargo test` on stable, in `velme-ir` and in `velme-wasm`.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use velme_ir::{CallNode, Goal, Origin, Request, from_json_str, validate};
use velme_sema::hir::{GoalId, Program};
use velme_wasm::EmitError;

/// The golden IR corpus's program: fuzzed IR is validated against each of its goals.
static PROGRAM: LazyLock<Option<Program>> = LazyLock::new(|| {
    let text = include_str!("../../tests/golden/ir/goals.velme");
    velme_sema::analyze(&velme_sema::SourceFile::new("goals.velme", text)).0
});

/// The compiler's call section of the one composite goal, taken from its golden IR (as `tests/validate.rs` does), so
/// joining a candidate's calls and comparing a complete goal's (stage 6) are fuzzed too.
static CALLS: LazyLock<Vec<CallNode>> = LazyLock::new(|| {
    from_json_str::<Goal>(include_str!("../../tests/golden/ir/accept/player_summary.json"))
        .map(|g| g.calls)
        .unwrap_or_default()
});

fuzz_target!(|data: &[u8]| {
    let (Ok(text), Some(program)) = (std::str::from_utf8(data), PROGRAM.as_ref()) else {
        return;
    };
    for (goal, declared) in program.goals.iter().enumerate() {
        let calls: &[CallNode] = if declared.name == "BuildPlayerSummary" { &CALLS } else { &[] };
        for origin in [Origin::Candidate, Origin::Complete] {
            let request = Request {
                program,
                goal: GoalId(goal),
                calls,
                origin,
            };
            if let Ok(ir) = validate(text, &request) {
                let emitted = velme_wasm::emit(&ir);
                assert!(!matches!(emitted, Err(EmitError::Internal(_))), "{emitted:?}");
            }
        }
    }
});
