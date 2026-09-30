//! The differential harness (`runtime/31` R-SBX-15, `delivery/51` §2, D-118): a leaf goal body run on the interpreter
//! and on WASM through the runtime's seam, which must give the same value or the same full diagnostic (code, message
//! and notes), and the same fuel and memory, with no run near a quarter of its Wasmtime fuel backstop, at its limits
//! or at the figures it spent. The corpus is the golden IR, the examples and the goals of [`crate::generate`].

use std::sync::Arc;

use velme_builtins::Value;
use velme_builtins::execution::{Interrupt, Limits, Spent};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Goal, ValidIr, calls, decode_str, encode_value, from_json_str};
use velme_runtime::{Backend, Wasm, eval_leaf};
use velme_sema::hir::{GoalId, Program};
use velme_wasm::backstop_fuel;

use crate::{example_cases, goal_id, program, read, repo, valid_ir};

/// The examples, each with where the hand-written IR of its goals is: a directory of `<Goal>.json`, or one file. The
/// one list, which every suite that runs the examples' IR reads.
pub const EXAMPLES: [(&str, &str); 7] = [
    ("examples/beginner/add.velme", "tests/fixtures/run/add.json"),
    ("examples/beginner/hello.velme", "tests/fixtures/run/hello.ir"),
    ("examples/beginner/find_badge.velme", "tests/fixtures/run/find_badge.ir"),
    (
        "examples/beginner/double_then_add_one.velme",
        "tests/fixtures/run/double_then_add_one.ir",
    ),
    (
        "examples/intermediate/player_summary.velme",
        "tests/fixtures/run/player_summary.ir",
    ),
    (
        "examples/games/level_summary.velme",
        "tests/fixtures/run/level_summary.ir",
    ),
    (
        "examples/professional/order_total.velme",
        "tests/fixtures/run/order_total.ir",
    ),
];

/// One leaf goal of the corpus and the inputs it is run on.
pub struct Leaf {
    /// Where its IR is, for messages.
    pub name: String,
    /// The checked program it is a goal of.
    pub program: Program,
    /// The goal.
    pub goal: GoalId,
    /// Its IR, validated.
    pub ir: ValidIr,
    /// Each case's inputs, one per input in declared order.
    pub cases: Vec<Vec<Value>>,
}

/// The `.json` files of `path`, or `path` itself if it is one, in name order.
fn documents(path: &str) -> Vec<std::path::PathBuf> {
    let path = repo(path);
    if path.is_file() {
        return vec![path];
    }
    let mut files: Vec<_> = std::fs::read_dir(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

/// Every leaf of the golden IR, on the cases of `tests/golden/ir/run/`, and every leaf of the examples, on its
/// `examples:`. A composite goal's tail is the interpreter's (R-SBX-17), so only leaves are here.
pub fn corpus() -> Vec<Leaf> {
    let mut leaves = Vec::new();
    let text = read(&repo("tests/golden/ir/goals.velme"));
    let golden = program(&text);
    for path in documents("tests/golden/ir/accept") {
        let document = read(&path);
        let parsed: Goal = from_json_str(&document).expect("an IR document");
        let id = goal_id(&golden, &parsed.goal);
        if !calls(&golden, id).expect("a checked goal").is_empty() {
            continue;
        }
        let file = path.file_name().and_then(|n| n.to_str()).expect("a file name");
        let cases: serde_json::Value =
            from_json_str(&read(&repo(&format!("tests/golden/ir/run/{file}")))).expect("the golden cases");
        let params = &golden.goals[id.0].params;
        let cases = cases
            .as_array()
            .expect("a list of cases")
            .iter()
            .map(|case| {
                params
                    .iter()
                    .map(|p| decode_str(&case["inputs"][&p.name].to_string(), &p.ty, &golden).expect("decodes"))
                    .collect()
            })
            .collect();
        leaves.push(Leaf {
            name: path.display().to_string(),
            program: golden.clone(),
            goal: id,
            ir: valid_ir(&golden, &document),
            cases,
        });
    }
    for (source, ir) in EXAMPLES {
        let text = read(&repo(source));
        let checked = program(&text);
        for path in documents(ir) {
            let document = read(&path);
            let parsed: Goal = from_json_str(&document).expect("an IR document");
            let id = goal_id(&checked, &parsed.goal);
            if !calls(&checked, id).expect("a checked goal").is_empty() {
                continue;
            }
            let cases = example_cases(&checked, &text, id)
                .into_iter()
                .map(|(inputs, _)| inputs)
                .collect();
            leaves.push(Leaf {
                name: format!("{source}: {}", parsed.goal),
                program: checked.clone(),
                goal: id,
                ir: valid_ir(&checked, &document),
                cases,
            });
        }
    }
    leaves
}

/// What a leaf body gave on one backend: its value or its failure's full diagnostic, and the fuel and memory spent.
pub type Outcome = (Result<Value, Diagnostic>, Spent);

/// The leaf `goal` of `program`, whose IR is `ir`, run on `inputs` within `limits` on both backends: the outcome if
/// they are the same, value and diagnostic and fuel and memory, and the WASM run came nowhere near its backstops;
/// otherwise what differs.
pub fn compare(
    wasm: &Arc<Wasm>,
    program: &Program,
    goal: GoalId,
    ir: &ValidIr,
    inputs: &[Value],
    limits: Limits,
) -> Result<Outcome, String> {
    let target = &program.goals[goal.0];
    let on = |backend: &Backend| eval_leaf(backend, target, ir, inputs.to_vec(), limits, None);
    let interpreted = on(&Backend::Interp);
    let compiled = on(&Backend::Wasm(Arc::clone(wasm)));
    let shown = |(result, spent): &Outcome| {
        let result = match result {
            Ok(value) => encode_value(value).unwrap_or_else(|_| format!("{value:?}")),
            Err(diagnostic) => format!("{diagnostic:?}"),
        };
        format!("{result} (fuel {}, memory {})", spent.fuel, spent.memory)
    };
    if interpreted != compiled {
        return Err(format!("interp: {}\n  wasm: {}", shown(&interpreted), shown(&compiled)));
    }
    let run = wasm
        .run(ir, inputs, limits, &Interrupt::new(|| false))
        .map_err(|unrun| format!("wasm didn't run it: {unrun:?}"))?;
    if run.backstop.is_some() || !run.wasmtime_fuel.within_a_quarter() {
        return Err(format!("the backstops: {:?}, {:?}", run.backstop, run.wasmtime_fuel));
    }
    // The backstop of the figures the run spent, not of its limits, which at the system caps leave K untested.
    // Its memory term is the bytes charged: every bulk copy writes a charged value, or a `map` result into its
    // frame on the way to its list, so no byte moves more than `MEMORY_MOVES` times, which M allows; the one other
    // copy, a `sort_by` order within scratch, and growth by the page are paid for by K and A.
    let spent = backstop_fuel(run.spent.fuel, run.spent.memory);
    if run.wasmtime_fuel.used > spent / 4 {
        return Err(format!(
            "{:?} over a quarter of {spent}, the backstop of {:?}",
            run.wasmtime_fuel, run.spent
        ));
    }
    Ok(interpreted)
}

/// [`compare`] at the system limits, then with one unit of fuel and one byte of memory less than that run spent,
/// so each deterministic limit stops both backends at the same point with the same figures (INV-3, D-118): the run
/// one unit of fuel short fails with `VL0601`, the one a byte of memory short with `VL0604`. The outcome within the
/// system limits, if all agree.
pub fn differential(
    wasm: &Arc<Wasm>,
    program: &Program,
    goal: GoalId,
    ir: &ValidIr,
    inputs: &[Value],
) -> Result<Outcome, String> {
    let outcome = compare(wasm, program, goal, ir, inputs, Limits::SYSTEM)?;
    let spent = outcome.1;
    let short = [
        spent
            .fuel
            .checked_sub(1)
            .map(|fuel| (Limits { fuel, ..Limits::SYSTEM }, Code::BudgetExceeded)),
        spent.memory.checked_sub(1).map(|memory| {
            let limits = Limits {
                memory,
                ..Limits::SYSTEM
            };
            (limits, Code::MemoryLimitExceeded)
        }),
    ];
    for (limits, code) in short.into_iter().flatten() {
        let (result, _) =
            compare(wasm, program, goal, ir, inputs, limits).map_err(|e| format!("within {limits:?}: {e}"))?;
        match result {
            Err(diagnostic) if diagnostic.code == code => {}
            other => return Err(format!("within {limits:?}: expected {code:?}, got {other:?}")),
        }
    }
    Ok(outcome)
}
