//! Domains at the CLI (`tooling/40` R-CLI-29, `reference/90` R-ERR-04): JSON input walked deep before any body runs,
//! outputs walked before their checks, on both backends, with their exit codes; and `velme check` holding source
//! literals to the whole condition (`language/11` R-TYP-31).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use velme_ir::wired_goal;
use velme_test_support::ir_json::{binary, input, literal};
use velme_test_support::{goal_id, install, program, velme_command};

const FILE: &str = "cards.velme";

const SOURCE: &str = "language: velme/0.2

type Score = Number where value >= 0
type Ratio = Number where 10 / value != 0
type Heavy = Number where value != 0 or (every i in range(10000) has every j in range(20) has i + j >= value)
type Pair where value.low <= value.high:
    low: Number
    high: Number
type Card:
    pair: Pair
    scores: List<Score?>

goal Total(card: Card) -> Number:
    plan: \"Divide the card's low by zero.\"

goal Shift(n: Number) -> Score:
    plan: \"Take one off n.\"
    check:
        - result >= 100

goal Divide(r: Ratio) -> Number:
    plan: \"Return r.\"

goal Busy(h: Heavy) -> Number:
    budget cpu=1ms
    plan: \"Return h.\"
";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn body(goal: &str) -> Value {
    match goal {
        // Divides the card's low by zero, so a `VL0902` shows it never ran.
        "Total" => {
            let pair = json!({"kind": "field", "of": input("card"), "field": "pair"});
            binary("div", json!({"kind": "field", "of": pair, "field": "low"}), literal(0))
        }
        "Shift" => binary("sub", input("n"), literal(1)),
        "Divide" => input("r"),
        "Busy" => input("h"),
        _ => panic!("no goal {goal}"),
    }
}

/// A project for `test` with every goal of [`SOURCE`] installed.
fn project(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("domains-{}", std::process::id()))
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    fs::write(dir.join(FILE), SOURCE).expect("source");
    let program = program(SOURCE);
    for goal in ["Total", "Shift", "Divide", "Busy"] {
        let mut ir = serde_json::to_value(wired_goal(&program, goal_id(&program, goal)).expect("IR")).expect("IR");
        ir["body"] = body(goal);
        install(&dir, FILE, &program, &ir.to_string());
    }
    dir
}

fn velme(dir: &Path, args: &[&str]) -> Out {
    let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn run(dir: &Path, goal: &str, args: &[&str], backend: &str) -> Out {
    velme(
        dir,
        &[&["run", FILE, "--goal", goal, "--backend", backend], args].concat(),
    )
}

/// AC-CLI-27, AC-ERR-05: an invalid nested domain value in JSON input exits 64 with `VL0902`, naming the domain, the
/// first path and a value error's cause, before any body runs, on both backends; base JSON values pass, `null` in an
/// optional item too.
#[test]
fn ac_cli_27_invalid_nested_json_input_exits_64_before_the_body() {
    let dir = project("ac_cli_27");
    for backend in ["interp", "wasm"] {
        let card = r#"card={"pair": {"low": 1, "high": 2}, "scores": [null, 3, -4]}"#;
        let out = run(&dir, "Total", &["--arg", card], backend);
        assert_eq!(out.code, 64, "{backend}: {}{}", out.stdout, out.stderr);
        assert!(
            out.stderr
                .contains("Input `card.scores[2]` is outside `Score`: it must be at least 0.  [VL0902]"),
            "{backend}: {}",
            out.stderr
        );
        let pair = r#"card={"pair": {"low": 3, "high": 2}, "scores": [-1]}"#;
        let out = run(&dir, "Total", &["--arg", pair], backend);
        assert_eq!(out.code, 64);
        assert!(
            out.stderr.contains("Input `card.pair` is outside `Pair`"),
            "{}",
            out.stderr
        );
        let out = run(&dir, "Divide", &["--arg", "r=0", "--json"], backend);
        assert_eq!(out.code, 64);
        let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
        let diag = &envelope["results"][0]["diagnostics"][0];
        assert_eq!(diag["code"], "VL0902");
        assert_eq!(
            diag["message"],
            "Input `r` is outside `Ratio`: `10 / value != 0` tried to divide 10 by 0, which has no answer."
        );
        assert_eq!(diag["notes"], json!(["cause: VL0602"]));
        // Valid base JSON runs the body, which divides by zero: exit 3.
        let fine = r#"card={"pair": {"low": 1, "high": 2}, "scores": [null, 0]}"#;
        let out = run(&dir, "Total", &["--arg", fine], backend);
        assert_eq!(out.code, 3, "{backend}: {}", out.stderr);
        assert!(out.stderr.contains("[VL0602]"), "{}", out.stderr);
        assert_eq!(run(&dir, "Divide", &["--arg", "r=5"], backend).code, 0);
    }
}

/// AC-CLI-28, AC-ERR-05: an output outside its domain exits 3 with `VL0504` before its checks, in the JSON envelope
/// too; a membership walk out of budget keeps `VL0601` and its exit.
#[test]
fn ac_cli_28_an_output_outside_its_domain_exits_3() {
    let dir = project("ac_cli_28");
    for backend in ["interp", "wasm"] {
        let out = run(&dir, "Shift", &["--arg", "n=0", "--json"], backend);
        assert_eq!(out.code, 3, "{}", out.stdout);
        let envelope: Value = serde_json::from_str(&out.stdout).expect("JSON");
        let diags = envelope["results"][0]["diagnostics"].as_array().expect("diagnostics");
        assert_eq!(diags.len(), 1, "no check ran after it: {diags:?}");
        assert_eq!(diags[0]["code"], "VL0504");
        assert_eq!(
            diags[0]["message"],
            "`Shift` returned a value outside `Score` at `result`: it must be at least 0."
        );
        let busy = run(&dir, "Busy", &["--arg", "h=0"], backend);
        assert_eq!(busy.code, 3, "{}", busy.stderr);
        assert!(busy.stderr.contains("[VL0601]"), "{}", busy.stderr);
        assert!(!busy.stderr.contains("VL0902"), "{}", busy.stderr);
    }
}

/// AC-TYP-30: `velme check` holds a source literal to the domain's whole condition, `VL0204` with exit 1, after the
/// static pass found nothing; a valid one passes.
#[test]
fn ac_typ_30_velme_check_rejects_a_literal_outside_a_residual_condition() {
    let dir = project("ac_typ_30");
    let source = SOURCE.replace(
        "goal Divide(r: Ratio) -> Number:\n    plan: \"Return r.\"\n",
        "goal Divide(r: Ratio) -> Number:\n    plan: \"Return r.\"\n    examples:\n        - Divide(0) == 0\n",
    );
    fs::write(dir.join(FILE), &source).expect("source");
    let out = velme(&dir, &["check", FILE]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains(
            "`0` isn't a valid `Ratio`: `10 / value != 0` tried to divide 10 by 0, which has no answer.  [VL0204]"
        ),
        "{}",
        out.stderr
    );
    fs::write(dir.join(FILE), source.replace("Divide(0) == 0", "Divide(5) == 5")).expect("source");
    let out = velme(&dir, &["check", FILE]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
}

/// AC-ERR-05: the human rendering of a deep input or output failure names the domain, the path and a value error's
/// cause; a predicate's budget failure is not relabelled as non-membership.
#[test]
fn ac_err_05_membership_failures_render_domain_path_and_cause() {
    let dir = project("ac_err_05");
    let divide = run(&dir, "Divide", &["--arg", "r=0"], "interp");
    assert!(
        divide.stderr.contains(
            "Error: Input `r` is outside `Ratio`: `10 / value != 0` tried to divide 10 by 0, which has no answer.  [VL0902]"
        ),
        "{}",
        divide.stderr
    );
    assert!(divide.stderr.contains("cause: VL0602"), "{}", divide.stderr);
    let card = r#"card={"pair": {"low": 1, "high": 2}, "scores": [-4]}"#;
    let total = run(&dir, "Total", &["--arg", card], "interp");
    assert!(
        total.stderr.contains("Input `card.scores[0]` is outside `Score`"),
        "{}",
        total.stderr
    );
    let shift = run(&dir, "Shift", &["--arg", "n=0"], "interp");
    assert!(
        shift
            .stderr
            .contains("Error: `Shift` returned a value outside `Score` at `result`: it must be at least 0.  [VL0504]"),
        "{}",
        shift.stderr
    );
    let busy = run(&dir, "Busy", &["--arg", "h=0"], "interp");
    assert!(
        busy.stderr.contains("[VL0601]") && !busy.stderr.contains("outside"),
        "{}",
        busy.stderr
    );
}

/// AC-CLI-29, AC-SYNTH-67, AC-CMP-09 (the probe): `velme check` probes every domain and reports a scarce one with
/// `VL0210` and exit 1; `velme build` stops on it before any provider is set up, where it would otherwise report that
/// none is (`VL0405`).
#[test]
fn ac_cli_29_velme_check_reports_a_scarce_domain_without_a_provider() {
    let dir = project("ac_cli_29");
    // 7 of the probe's 64 draws are 4 or less (`compiler/22` R-SYNTH-66's threshold fixture).
    let scarce =
        "type Rare = Number where value >= 1 and value <= 64 and value is whole and (value <= 4 or value > 1000)\n";
    let source = SOURCE.replace("type Card:\n", &format!("{scarce}type Card:\n"));
    fs::write(dir.join(FILE), &source).expect("source");
    let out = velme(&dir, &["check", FILE]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.contains(
            "Error: Domain `Rare` produced only 7/64 valid values; at least 8 are needed. Loosen its condition.  [VL0210]"
        ),
        "{}",
        out.stderr
    );
    let build = velme(&dir, &["build", FILE]);
    assert_eq!(build.code, 1, "{}{}", build.stdout, build.stderr);
    assert!(build.stderr.contains("[VL0210]"), "{}", build.stderr);
    assert!(
        !build.stderr.contains("VL0405"),
        "no provider was looked for: {}",
        build.stderr
    );
    // At 8 of 64 it passes.
    fs::write(dir.join(FILE), source.replace("value <= 4 or", "value <= 6 or")).expect("source");
    let out = velme(&dir, &["check", FILE]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
}

/// AC-SYNTH-67, AC-CMP-02, F4 of D5: `velme build` on a scarce domain constructs no provider: with none configured it
/// reports the domain alone, while the same build with no scarce domain goes on to look for one (`VL0405`).
#[test]
fn ac_synth_67_build_with_a_scarce_domain_constructs_no_provider() {
    let dir = project("ac_synth_67_build");
    let scarce =
        "type Rare = Number where value >= 1 and value <= 64 and value is whole and (value <= 4 or value > 1000)\n";
    let stale = SOURCE.replace("plan: \"Return h.\"", "plan: \"Give back h.\"");
    let build = |source: &str| {
        fs::write(dir.join(FILE), source).expect("source");
        let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
            .args(["build", FILE])
            .current_dir(&dir)
            .env("NO_COLOR", "1")
            .env_remove("VELME_SYNTH_SCRIPT")
            .env_remove("VELME_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .output()
            .expect("velme runs");
        String::from_utf8(out.stderr).expect("utf-8")
    };
    let reported = build(&stale.replace("type Card:\n", &format!("{scarce}type Card:\n")));
    assert!(reported.contains("[VL0210]"), "{reported}");
    assert!(!reported.contains("[VL0405]"), "{reported}");
    let reached = build(&stale);
    assert!(reached.contains("[VL0405]"), "{reached}");
    assert!(!reached.contains("[VL0210]"), "{reached}");
    assert!(!dir.join(".velme/synth-log.jsonl").exists(), "nothing was requested");
}

/// AC-CLI-29, AC-SYNTH-67: a predicate out of budget in `velme check`'s probe is no scarcity: `VL0601` at the domain,
/// keeping its runtime exit code (R-CLI-29).
#[test]
fn ac_cli_29_a_probe_budget_stop_keeps_its_runtime_exit_code() {
    let dir = project("ac_cli_29_budget");
    let busy = "type Slow = Number where every i in range(10000) has every j in range(10000) has i + j >= value\n";
    fs::write(
        dir.join(FILE),
        SOURCE.replace("type Card:\n", &format!("{busy}type Card:\n")),
    )
    .expect("source");
    let out = velme(&dir, &["check", FILE]);
    assert_eq!(out.code, 3, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr
            .contains("Error: Checking whether a value is a `Slow` took too many steps.  [VL0601]"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("make its `where` condition cheaper"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("VL0210"), "{}", out.stderr);
}
