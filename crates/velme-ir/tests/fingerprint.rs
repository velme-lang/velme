//! Fingerprints (`runtime/32` §2, D-11, D-26): signatures, `contract_key`, `synthesis_key` and `execution_id`.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use serde_json::json;
use velme_ir::{
    CallNode, Fingerprint, Goal, Synthesis, compatibility, contract_key, execution_id, from_json_str, signature,
    synthesis_key, to_canonical_string,
};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn program(text: &str) -> Program {
    let (program, diags) = analyze(&SourceFile::new("test.velme", text));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(
        program
            .goals
            .iter()
            .position(|g| g.name == name)
            .unwrap_or_else(|| panic!("no goal {name}")),
    )
}

fn sig(program: &Program, goal: &str) -> Fingerprint {
    signature(program, goal_id(program, goal)).expect("signature")
}

fn key(program: &Program, goal: &str) -> Fingerprint {
    contract_key(program, goal_id(program, goal)).expect("contract key")
}

const SUMMARY: &str = r#"language: velme/0.1

type Badge:
    label: Text

type Player:
    name: Text
    score: Number
    badge: Badge?

type Summary:
    score: Number
    player: Player

goal CalculateScore(player: Player) -> Number:
    plan: "Return the player's score."
    check:
        - result == player.score

goal Describe(label: Text) -> Text:
    plan: "Repeat the label."

goal BuildSummary(player: Player) -> Summary:
    call:
        score = CalculateScore(player)
    plan: "Build the summary."
    check:
        - result.score == score
        - every p in [player] has p.score >= 0
    examples:
        - BuildSummary(Player(name: "Lina", score: 820, badge: nothing)) == Summary(score: 820, player: Player(name: "Lina", score: 820, badge: nothing))
"#;

/// `SUMMARY` with the first `from` replaced by `to`.
fn edited(from: &str, to: &str) -> Program {
    assert!(SUMMARY.contains(from), "{from}");
    program(&SUMMARY.replacen(from, to, 1))
}

// ---- the `b3:` form ----

#[test]
fn blake3_golden_vectors_of_r_ir_21() {
    for (json, hex) in [
        (
            r#"{"a":3,"b":1,"é":2}"#,
            "7d56b0352359754359630cf7b7b84db97df2d79f02a5ae541912a1dee18688d6",
        ),
        (
            r#"{"s":"line1\nline2\ttab\"quoteé"}"#,
            "0856a95de184ff551d749d960cbe5f544945772b29a30393420f339d80449862",
        ),
        (
            r#"{"n":0.1}"#,
            "5b7a23a0b8b66250921682288e8176595b285f99ee4cf48fdb74d94ce1d23f3d",
        ),
    ] {
        let value: serde_json::Value = from_json_str(json).expect("parses");
        assert_eq!(to_canonical_string(&value).expect("canonical"), json);
        let fingerprint = Fingerprint::of(&value).expect("hashes");
        assert_eq!(fingerprint.hex(), hex);
        assert_eq!(fingerprint.to_string(), format!("b3:{hex}"));
        assert_eq!(fingerprint.to_string().parse::<Fingerprint>(), Ok(fingerprint));
    }
}

/// R-IR-21's canonical form is JCS-like, not JCS: keys sort by code point (UTF-8 bytes), not UTF-16 code units, so
/// `｡` (U+FF61) sorts before `😀` (U+1F600, the surrogates D83D DE00); and numbers are plain decimals (R-TYP-08), not
/// ES6 renderings such as `1e+21` and `1e-7`.
#[test]
fn canonical_json_is_jcs_like_not_jcs() {
    for (input, canonical, hex) in [
        (
            r#"{"😀":2,"｡":1}"#,
            r#"{"｡":1,"😀":2}"#,
            "899b786e1c7b7f49e7315b2211771e725f2fd047cf784a9400d66bca18e9f843",
        ),
        (
            r#"{"n":1e21}"#,
            r#"{"n":1000000000000000000000}"#,
            "9fc33899cf05af9c90c9aceb75c5b16c651ecb9e0cc703147a574ea8d530e185",
        ),
        (
            r#"{"n":1e-7}"#,
            r#"{"n":0.0000001}"#,
            "a06f5c50a25acd77e208ba662c6054caba0776076d697837e4cb0984fe154db9",
        ),
    ] {
        let value: serde_json::Value = from_json_str(input).expect("parses");
        assert_eq!(to_canonical_string(&value).expect("canonical"), canonical);
        assert_eq!(Fingerprint::of(&value).expect("hashes").hex(), hex, "{canonical}");
    }
}

/// The `contract_key` of one fixed goal, spelled out: the D-81 document is built by hand, so a change to a Rust
/// type's serialization can't reach it. Changing this value invalidates every lock.
#[test]
fn contract_key_golden_value() {
    assert_eq!(
        key(&program(SUMMARY), "CalculateScore").to_string(),
        "b3:f64dd8797d69b05fdf869357971f3b0b622346ac52127985659a2e6e33d33cfb"
    );
}

/// D-85: the compatibility unit is `MAJOR`, or `MAJOR.MINOR` while `MAJOR` is 0, as Cargo reads versions.
#[test]
fn compatibility_units_follow_cargo() {
    for (version, unit) in [
        ("0.1", "0.1"),
        ("0.1.4", "0.1"),
        ("1.2", "1"),
        ("12.0.3", "12"),
        ("0", "0"),
    ] {
        assert_eq!(compatibility(version), unit, "{version}");
    }
}

#[test]
fn fingerprints_have_one_written_form() {
    let fingerprint = Fingerprint::of_bytes(b"velme");
    let text = fingerprint.to_string();
    assert_eq!(serde_json::to_value(fingerprint).expect("serializes"), json!(text));
    assert_eq!(
        from_json_str::<Fingerprint>(&format!("\"{text}\"")).expect("parses"),
        fingerprint
    );
    for bad in [
        text.to_uppercase(),
        text.replacen("b3:", "B3:", 1),
        text.replacen("b3:", "", 1),
        text.replacen("b3:", "b2:", 1),
        text[..text.len() - 1].to_owned(),
        format!("{text}0"),
        format!("{} ", text),
    ] {
        assert!(bad.parse::<Fingerprint>().is_err(), "{bad}");
        assert!(from_json_str::<Fingerprint>(&format!("\"{bad}\"")).is_err(), "{bad}");
    }
}

// ---- signature ----

/// The golden composite's `goal_signature`s are its children's real signatures (`compiler/21` R-IR-09).
#[test]
fn golden_ir_call_signatures_are_the_children_signatures() {
    let program = program(&read(&repo("tests/golden/ir/goals.velme")));
    let mut checked = 0;
    insta::glob!("../../../tests/golden/ir/accept", "*.json", |path| {
        let goal: Goal = from_json_str(&read(path)).expect("golden parses");
        for CallNode::Call(call) in &goal.calls {
            assert_eq!(
                call.goal_signature,
                sig(&program, &call.goal).to_string(),
                "{}: call `{}`",
                path.display(),
                call.binding
            );
            checked += 1;
        }
    });
    assert!(checked > 0, "no golden IR has calls");
}

/// `runtime/32` §2: the goal name, the inputs and output, and every record type they reach, in IR type form.
#[test]
fn signature_hashes_name_inputs_output_and_record_types() {
    let program = program(&read(&repo("tests/golden/ir/goals.velme")));
    let player =
        json!({"fields": [["name", {"t": "Text"}], ["jump_height", {"t": "Number"}], ["score", {"t": "Number"}]]});
    let doc = json!({
        "goal": "CalculateScore",
        "inputs": [["player", {"t": "Record", "name": "Player"}]],
        "output": {"t": "Number"},
        "types": {"Player": player},
    });
    assert_eq!(sig(&program, "CalculateScore"), Fingerprint::of(&doc).expect("hashes"));
}

/// Pins the key documents: a changed value here changes every stored key, so it needs a reason (R-ART-04).
#[test]
fn golden_goal_fingerprints() {
    let program = program(&read(&repo("tests/golden/ir/goals.velme")));
    let keys: Vec<String> = program
        .goals
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let id = GoalId(i);
            format!(
                "{}\n  signature    {}\n  contract_key {}",
                g.name,
                signature(&program, id).expect("signature"),
                contract_key(&program, id).expect("contract key")
            )
        })
        .collect();
    insta::assert_snapshot!(keys.join("\n"));
}

#[test]
fn signature_covers_name_inputs_output_and_reachable_records() {
    let base = program(SUMMARY);
    // A record reached only through a field (`Player.badge`) is part of the signature of every goal that reaches it.
    for (from, to) in [
        ("    label: Text\n", "    label: Text\n    tier: Number\n"),
        ("    label: Text\n", "    title: Text\n"),
        ("    label: Text\n", "    label: Number\n"),
    ] {
        let changed = edited(from, to);
        for goal in ["CalculateScore", "BuildSummary"] {
            assert_ne!(sig(&base, goal), sig(&changed, goal), "{goal}: {to}");
        }
        assert_eq!(sig(&base, "Describe"), sig(&changed, "Describe"), "{to}");
    }
    let describe = sig(&base, "Describe");
    assert_ne!(
        describe,
        sig(&edited("goal Describe(label", "goal Describe(text"), "Describe")
    );
    assert_ne!(
        describe,
        sig(&edited("(label: Text) -> Text", "(label: Text) -> Text?"), "Describe")
    );
    assert_ne!(
        describe,
        sig(&edited("(label: Text) -> Text", "(label: Text?) -> Text"), "Describe")
    );
    assert_ne!(describe, sig(&edited("goal Describe", "goal Repeat"), "Repeat"));
}

// ---- contract_key ----

#[test]
fn signature_ignores_plan_checks_and_examples_contract_key_does_not() {
    let base = program(SUMMARY);
    for (from, to) in [
        ("Build the summary.", "Build the summary, carefully."),
        ("result.score == score", "result.score >= score"),
        ("        - every p in [player] has p.score >= 0\n", ""),
        ("score: 820, player", "score: 821, player"),
        ("Summary:\n    call:", "Summary:\n    budget cpu=10ms\n    call:"),
    ] {
        let changed = edited(from, to);
        assert_eq!(sig(&base, "BuildSummary"), sig(&changed, "BuildSummary"), "{to}");
        assert_ne!(key(&base, "BuildSummary"), key(&changed, "BuildSummary"), "{to}");
    }
}

#[test]
fn contract_key_ignores_layout_comments_and_variable_names() {
    let base = program(SUMMARY);
    for (from, to) in [
        ("goal Describe", "# A comment.\n\ngoal Describe"),
        ("result.score == score", "result.score==score  # the bound score"),
        (
            "every p in [player] has p.score >= 0",
            "every q in [player] has q.score >= 0",
        ),
        ("score: 820, player", "score: 820.0, player"),
        ("score: 820, player", "score: 0820, player"),
        ("score: 820, player", "score: 8_20, player"),
    ] {
        let changed = edited(from, to);
        assert_eq!(key(&base, "BuildSummary"), key(&changed, "BuildSummary"), "{to}");
    }
}

/// R-ART-02, D-11: a parent keys on its child's signature, so a new plan for the child changes the child's
/// `contract_key` only.
#[test]
fn child_contract_change_leaves_parent_keys_alone() {
    let base = program(SUMMARY);
    let changed = edited("Return the player's score.", "Return the score.");
    assert_ne!(key(&base, "CalculateScore"), key(&changed, "CalculateScore"));
    assert_eq!(sig(&base, "CalculateScore"), sig(&changed, "CalculateScore"));
    assert_eq!(key(&base, "BuildSummary"), key(&changed, "BuildSummary"));
    // A child whose signature changes does change the parent's key.
    let output = edited("-> Number:\n    plan: \"Return", "-> Number?:\n    plan: \"Return");
    assert_ne!(key(&base, "BuildSummary"), key(&output, "BuildSummary"));
}

#[test]
fn keys_do_not_depend_on_declaration_order() {
    let base = program(SUMMARY);
    let moved = SUMMARY.replacen(
        "goal Describe(label: Text) -> Text:\n    plan: \"Repeat the label.\"\n\n",
        "",
        1,
    ) + "\ngoal Describe(label: Text) -> Text:\n    plan: \"Repeat the label.\"\n";
    let moved = program(&moved);
    for goal in ["CalculateScore", "Describe", "BuildSummary"] {
        assert_eq!(sig(&base, goal), sig(&moved, goal), "{goal}");
        assert_eq!(key(&base, goal), key(&moved, goal), "{goal}");
    }
}

// ---- synthesis_key and execution_id ----

#[test]
fn synthesis_key_takes_the_compiler_major_minor_only() {
    let contract = key(&program(SUMMARY), "BuildSummary");
    let base = Synthesis {
        input_version: "leaf-v1",
        compiler_version: "0.1.4",
        provider: "anthropic",
        model: "model-a",
    };
    let at = |s: Synthesis<'_>| synthesis_key(contract, &s).expect("synthesis key");
    // R-ART-04: a patch release changes no key.
    assert_eq!(
        at(base),
        at(Synthesis {
            compiler_version: "0.1.9",
            ..base
        })
    );
    assert_eq!(
        at(base),
        at(Synthesis {
            compiler_version: "0.1.0-rc.1",
            ..base
        })
    );
    // R-ART-03: everything else a synthesis run adds is part of the store key.
    for other in [
        Synthesis {
            compiler_version: "0.2.0",
            ..base
        },
        Synthesis {
            input_version: "leaf-v2",
            ..base
        },
        Synthesis {
            provider: "ollama",
            ..base
        },
        Synthesis {
            model: "model-b",
            ..base
        },
    ] {
        assert_ne!(at(base), at(other), "{other:?}");
    }
    let other_contract = key(&program(SUMMARY), "CalculateScore");
    assert_ne!(at(base), synthesis_key(other_contract, &base).expect("synthesis key"));
}

#[test]
fn execution_id_follows_the_child_artifacts() {
    let [parent, a, b] = ["parent", "child-a", "child-b"].map(|s| Fingerprint::of_bytes(s.as_bytes()));
    let id = execution_id(parent, &[a, b]);
    // Built by hand, it is still the canonical JSON of its document.
    let doc = json!({"artifact": parent, "children": [a, b]});
    assert_eq!(id, Fingerprint::of(&doc).expect("hashes"));
    assert_ne!(id, execution_id(parent, &[b, a]));
    assert_ne!(id, execution_id(parent, &[a, parent]));
    assert_ne!(execution_id(parent, &[]), execution_id(a, &[]));
}
