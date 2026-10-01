//! The `--json` contract (`tooling/40` R-CLI-15, D-108): `docs/schemas/velme-cli-1.schema.json` against every `--json`
//! snapshot the CLI tests keep, and against a failing check's envelope, whose message is the human one.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::Path;
use velme_test_support::velme_command;

use serde_json::Value;
use velme_test_support::repo;
use velme_test_support::schema::{assert_cli_envelope, violations};

/// The body of an insta snapshot file: what follows its header.
fn body(snapshot: &str) -> &str {
    let rest = snapshot.strip_prefix("---\n").unwrap_or(snapshot);
    rest.split_once("\n---\n").map_or(rest, |(_, body)| body)
}

/// The golden suite is every `--json` snapshot in `velme-cli`'s tests, and each validates against the schema, so the schema
/// and the real output can't drift apart (AC-CLI-12, R-CLI-15, D-108).
#[test]
fn ac_cli_12_every_json_snapshot_validates_against_the_schema() {
    let dir = repo("crates/velme-cli/tests/snapshots");
    let mut envelopes = 0;
    for entry in fs::read_dir(&dir).expect("snapshots") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "snap") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("snapshot");
        let body = body(&text).trim_start();
        if !body.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            assert!(
                !body.contains("velme-cli/1"),
                "{} looks like an envelope but isn't JSON",
                path.display()
            );
            continue;
        };
        if value.get("format").and_then(Value::as_str) != Some("velme-cli/1") {
            continue;
        }
        envelopes += 1;
        assert_cli_envelope(&value);
    }
    assert!(envelopes >= 60, "only {envelopes} envelope snapshots were found");
}

/// The schema rejects what it should, so a pass above means something.
#[test]
fn the_schema_rejects_a_malformed_envelope() {
    let schema: Value =
        serde_json::from_str(&fs::read_to_string(repo("docs/schemas/velme-cli-1.schema.json")).expect("schema"))
            .expect("JSON");
    let bad = |envelope: Value| !violations(&schema, &envelope).is_empty();
    let ok =
        serde_json::json!({"format": "velme-cli/1", "status": "ok", "results": [], "diagnostics": [], "notices": []});
    assert!(!bad(ok.clone()));
    assert!(bad(
        serde_json::json!({"format": "velme-cli/2", "status": "ok", "results": [], "diagnostics": [], "notices": []})
    ));
    assert!(bad(
        serde_json::json!({"format": "velme-cli/1", "status": "fine", "results": [], "diagnostics": [], "notices": []})
    ));
    let mut missing = ok.clone();
    missing.as_object_mut().expect("object").remove("notices");
    assert!(bad(missing));
    let mut extra = ok;
    extra["future"] = serde_json::json!(1);
    assert!(!bad(extra), "additions within velme-cli/1 are allowed");
    let failing = serde_json::json!({"format": "velme-cli/1", "status": "failed", "notices": [], "diagnostics": [],
        "results": [{"goal": "G", "status": "failed", "diagnostics": [{"code": "V1", "severity": "error", "message": "m",
        "file": "f", "span": {"start": 0, "end": 0, "line": 1, "column": 1}, "labels": [], "notes": []}]}]});
    assert!(bad(failing));
}

/// An artifact object without a manifest `format`, or without its `ir`, fails the schema (D-111).
#[test]
fn the_schema_rejects_an_artifact_missing_its_format_or_ir() {
    let schema: Value =
        serde_json::from_str(&fs::read_to_string(repo("docs/schemas/velme-cli-1.schema.json")).expect("schema"))
            .expect("JSON");
    let hash = format!("b3:{}", "0".repeat(64));
    let artifact = serde_json::json!({"artifact": hash, "ir": {"goal": "G"}, "manifest": {
        "format": "velme-artifact/1", "goal": "G", "kind": "leaf", "signature": hash, "contract_key": hash,
        "synthesis_key": hash, "language_version": "velme/0.1", "compiler_version": "0", "ir_version": "1",
        "builtins_version": "1", "provider": "scripted", "children": [],
        "verification": {"examples": 0, "generated_inputs": 0, "input_set": hash, "max_fuel_observed": 0}}});
    let envelope = |artifact: &Value| {
        serde_json::json!({"format": "velme-cli/1", "status": "ok", "notices": [], "diagnostics": [],
            "results": [{"goal": "G", "status": "ok", "diagnostics": [], "artifact": artifact}]})
    };
    assert!(violations(&schema, &envelope(&artifact)).is_empty());
    let mut no_format = artifact.clone();
    no_format["manifest"]
        .as_object_mut()
        .expect("manifest")
        .remove("format");
    assert!(!violations(&schema, &envelope(&no_format)).is_empty());
    let mut no_ir = artifact.clone();
    no_ir.as_object_mut().expect("artifact").remove("ir");
    assert!(!violations(&schema, &envelope(&no_ir)).is_empty());
    // The enums are closed: a status this schema does not list is refused.
    let mut wrong = envelope(&artifact);
    wrong["results"][0]["status"] = serde_json::json!("done");
    assert!(!violations(&schema, &wrong).is_empty(), "the status enum is closed");
}

/// A failing check under `--json` validates against the schema and carries the human message's text (AC-CLI-05, R-CLI-08).
#[test]
fn ac_cli_05_a_failing_check_in_json_has_the_same_message_as_human_mode() {
    let dir = repo("tests/fixtures/run/add_broken");
    let run = |json: bool| {
        let mut args = vec!["run", "add.velme", "--goal", "Add", "--arg", "a=2", "--arg", "b=3"];
        if json {
            args.push("--json");
        }
        velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
            .args(args)
            .current_dir(Path::new(&dir))
            .env_remove("NO_COLOR")
            .output()
            .expect("velme runs")
    };
    let json = run(true);
    assert_eq!(json.status.code(), Some(3));
    let envelope: Value = serde_json::from_slice(&json.stdout).expect("one JSON document");
    assert_cli_envelope(&envelope);
    let diagnostic = &envelope["results"][0]["diagnostics"][0];
    assert_eq!(diagnostic["code"], "VL0501");
    let message = diagnostic["message"].as_str().expect("a message");
    let human = String::from_utf8(run(false).stderr).expect("UTF-8");
    assert!(
        human.contains(message),
        "the human output has the JSON message:\n{human}"
    );
    assert!(human.contains("[VL0501]"));
}
