//! The artifact document and the local store (`runtime/32` §3–4).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use velme_diagnostics::{Code, Span};
use velme_ir::{
    CallNode, Fingerprint, IR_VERSION, Synthesis, ValidIr, contract_key, from_json_str, signature, synthesis_key,
    to_canonical_string,
};
use velme_runtime::{ARTIFACTS_DIR, Artifact, Child, LoadError, Manifest, Store, TMP_DIR, VELME_DIR, Verification};
use velme_sema::hir::{GoalKind, Program};
use velme_test_support::{goal_id, program, read, repo, valid_ir};

/// An empty project directory for one test.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("store").join(name);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

fn goals() -> Program {
    program(&read(&repo("tests/golden/ir/goals.velme")))
}

fn golden(program: &Program, name: &str) -> ValidIr {
    valid_ir(program, &read(&repo(&format!("tests/golden/ir/accept/{name}.json"))))
}

/// The manifest of `ir`, as synthesis would write it for a leaf or composite goal.
fn manifest(program: &Program, ir: &ValidIr) -> Manifest {
    let goal = ir.goal();
    let id = goal_id(program, &goal.goal);
    let contract = contract_key(program, id).expect("contract key");
    let synthesis = Synthesis {
        input_version: "leaf-v1",
        compiler_version: "0.1.4",
        provider: "anthropic",
        model: "model-a",
    };
    Manifest {
        goal: goal.goal.clone(),
        kind: program.goals[id.0].kind,
        signature: signature(program, id).expect("signature"),
        contract_key: contract,
        synthesis_key: synthesis_key(contract, &synthesis).expect("synthesis key"),
        language_version: program.language_version.clone(),
        compiler_version: synthesis.compiler_version.to_owned(),
        ir_version: goal.ir_version.clone(),
        builtins_version: goal.builtins_version.clone(),
        prompt_version: Some(synthesis.input_version.to_owned()),
        provider: synthesis.provider.to_owned(),
        backend: None,
        model_version: Some(synthesis.model.to_owned()),
        children: goal
            .calls
            .iter()
            .map(|CallNode::Call(call)| Child {
                binding: call.binding.clone(),
                goal: call.goal.clone(),
                signature: call.goal_signature.parse().expect("a real signature"),
            })
            .collect(),
        verification: Verification {
            examples: 3,
            generated_inputs: 61,
            input_set: Fingerprint::of_bytes(b"input set"),
            max_fuel_observed: 412,
        },
    }
}

/// A stored composite artifact: its store, id and document.
fn stored(name: &str) -> (Store, Fingerprint, Artifact) {
    let program = goals();
    let ir = golden(&program, "player_summary");
    let manifest = manifest(&program, &ir);
    let store = Store::new(&project(name));
    let id = store.put(&manifest, &ir).expect("stored");
    let artifact = Artifact {
        manifest,
        ir: ir.into_goal(),
    };
    (store, id, artifact)
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("store directory")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- criteria ----

#[test]
fn ac_art_06_tampered_artifact_is_corrupt_and_deleted_one_unavailable() {
    let (store, id, artifact) = stored("ac_art_06");
    assert_eq!(store.get(id).expect("loads"), artifact);
    let path = store.path(id);
    let text = read(&path).replacen("\"examples\":3", "\"examples\":4", 1);
    fs::write(&path, &text).expect("tampered");
    let error = store.get(id).expect_err("tampered");
    assert!(matches!(error, LoadError::Corrupt { found, .. } if found == Fingerprint::of_bytes(text.as_bytes())));
    let diag = error.diagnostic("BuildPlayerSummary", Span::default());
    assert_eq!(diag.code, Code::ArtifactCorrupt);
    assert_eq!(
        diag.message,
        "The built version of `BuildPlayerSummary` was changed or damaged."
    );
    fs::remove_file(&path).expect("deleted");
    let error = store.get(id).expect_err("deleted");
    assert!(matches!(error, LoadError::Unavailable { .. }), "{error:?}");
    let diag = error.diagnostic("BuildPlayerSummary", Span::default());
    assert_eq!(diag.code, Code::ArtifactUnavailable);
    assert_eq!(
        diag.message,
        "The built version of `BuildPlayerSummary` is missing — run `velme build`."
    );
}

/// Any single-byte edit, anywhere in the file, is caught by the hash (T-4).
#[test]
fn ac_sec_04_editing_one_byte_of_an_artifact_is_corrupt() {
    let (store, id, _) = stored("ac_sec_04");
    let path = store.path(id);
    let bytes = fs::read(&path).expect("stored bytes");
    for at in 0..bytes.len() {
        let mut edited = bytes.clone();
        edited[at] ^= 0x01;
        fs::write(&path, &edited).expect("edited");
        let error = store.get(id).expect_err("edited");
        assert_eq!(error.code(), Code::ArtifactCorrupt, "byte {at}");
    }
}

/// R-ART-05: every field of the document is listed here (and `backend`, which only an external artifact has); none
/// records when, where or by whom it was built, and a document with any other field is rejected.
#[test]
fn ac_art_08_artifact_documents_hold_no_timestamps_users_or_hosts() {
    let (store, id, _) = stored("ac_art_08");
    let document: Value = from_json_str(&read(&store.path(id))).expect("parses");
    let manifest = document["manifest"].as_object().expect("manifest object");
    let keys: BTreeSet<&str> = manifest.keys().map(String::as_str).collect();
    let expected = BTreeSet::from([
        "goal",
        "kind",
        "signature",
        "contract_key",
        "synthesis_key",
        "language_version",
        "compiler_version",
        "ir_version",
        "builtins_version",
        "prompt_version",
        "provider",
        "model_version",
        "children",
        "verification",
    ]);
    assert_eq!(keys, expected);
    let nested: BTreeSet<&str> = [&manifest["children"][0], &manifest["verification"]]
        .into_iter()
        .flat_map(|v| v.as_object().expect("object").keys().map(String::as_str))
        .collect();
    let expected = BTreeSet::from([
        "binding",
        "goal",
        "signature",
        "examples",
        "generated_inputs",
        "input_set",
        "max_fuel_observed",
    ]);
    assert_eq!(nested, expected);
    let top: BTreeSet<&str> = document
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(top, BTreeSet::from(["manifest", "ir"]));
    // Adding a field anywhere in the manifest makes the document invalid, even under a matching name.
    for pointer in ["", "/manifest", "/manifest/verification", "/manifest/children/0"] {
        let mut edited = document.clone();
        edited
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .expect("object")
            .insert("built_at".to_owned(), Value::from("2026-09-29T10:00:00Z"));
        let bytes = to_canonical_string(&edited).expect("canonical");
        let forged = Fingerprint::of_bytes(bytes.as_bytes());
        fs::write(store.path(forged), &bytes).expect("written");
        let error = store.get(forged).expect_err("rejected");
        assert!(matches!(error, LoadError::Malformed { .. }), "{pointer}: {error:?}");
        assert_eq!(error.code(), Code::LockStale);
    }
}

// ---- document and store ----

/// The artifact document as stored, pretty-printed for review (`runtime/32` §3).
#[test]
fn artifact_document() {
    let (store, id, _) = stored("artifact_document");
    let document: Value = from_json_str(&read(&store.path(id))).expect("parses");
    insta::assert_snapshot!(serde_json::to_string_pretty(&document).expect("serializes"));
}

/// R-ART-05, R-ART-09: the file is the document's canonical JSON, named by its hash, the same in any project.
#[test]
fn stores_are_deterministic_and_content_addressed() {
    let (store, id, artifact) = stored("deterministic_a");
    let (other, other_id, _) = stored("deterministic_b");
    assert_eq!(id, other_id);
    let bytes = fs::read(store.path(id)).expect("stored bytes");
    assert_eq!(bytes, fs::read(other.path(id)).expect("stored bytes"));
    assert_eq!(Fingerprint::of_bytes(&bytes), id);
    assert_eq!(
        String::from_utf8(bytes).expect("UTF-8"),
        to_canonical_string(&artifact).expect("canonical")
    );
    assert!(
        store.dir().ends_with(Path::new(VELME_DIR).join(ARTIFACTS_DIR)),
        "{}",
        store.dir().display()
    );
}

#[test]
fn stores_write_once_and_leave_no_temporary_files() {
    let program = goals();
    let store = Store::new(&project("write_once"));
    let leaf = golden(&program, "find_badge");
    let composite = golden(&program, "player_summary");
    let leaf_id = store.put(&manifest(&program, &leaf), &leaf).expect("stored");
    let composite_id = store.put(&manifest(&program, &composite), &composite).expect("stored");
    assert_ne!(leaf_id, composite_id);
    // Temporaries are written beside the store, never in it, and removed once placed.
    let tmp = store.dir().parent().expect("inside .velme").join(TMP_DIR);
    assert_eq!(files(&tmp), Vec::<String>::new());
    let mut names = vec![
        format!("b3-{}.json", leaf_id.hex()),
        format!("b3-{}.json", composite_id.hex()),
    ];
    names.sort();
    assert_eq!(files(store.dir()), names);
    // R-ART-09: a file already under the name is never rewritten, even a damaged one.
    fs::write(store.path(leaf_id), "damaged").expect("damaged");
    assert_eq!(store.put(&manifest(&program, &leaf), &leaf).expect("stored"), leaf_id);
    assert_eq!(read(&store.path(leaf_id)), "damaged");
    assert_eq!(files(store.dir()), names);
}

#[test]
fn a_document_matching_its_name_must_still_be_an_artifact() {
    let store = Store::new(&project("malformed"));
    fs::create_dir_all(store.dir()).expect("store directory");
    for bytes in [&b"{}"[..], b"not json", b"\xff\xfe"] {
        let id = Fingerprint::of_bytes(bytes);
        fs::write(store.path(id), bytes).expect("written");
        let error = store.get(id).expect_err("malformed");
        assert!(matches!(error, LoadError::Malformed { .. }), "{error:?}");
        let diag = error.diagnostic("FindBadge", Span::default());
        assert_eq!(diag.code, Code::LockStale);
        assert_eq!(
            diag.message,
            "`FindBadge` changed since it was last built — run `velme build`."
        );
    }
}

/// R-ART-07: a wired goal's artifact records the compiler as its provider and has no model or prompt version.
#[test]
fn optional_manifest_fields_are_left_out() {
    let program = goals();
    let ir = golden(&program, "find_badge");
    let manifest = Manifest {
        kind: GoalKind::Wired,
        prompt_version: None,
        provider: "compiler".to_owned(),
        model_version: None,
        ..manifest(&program, &ir)
    };
    let store = Store::new(&project("optional_fields"));
    let id = store.put(&manifest, &ir).expect("stored");
    let text = read(&store.path(id));
    assert!(
        text.contains(r#""kind":"wired""#) && text.contains(r#""provider":"compiler""#),
        "{text}"
    );
    for field in ["model_version", "prompt_version", "backend"] {
        assert!(!text.contains(field), "{field}: {text}");
    }
    let loaded = store.get(id).expect("loads");
    assert_eq!(loaded.manifest, manifest);
    assert_eq!(loaded.ir.ir_version, IR_VERSION);
}

/// R-ART-21: an external backend's artifact names the backend.
#[test]
fn external_artifacts_name_their_backend() {
    let program = goals();
    let ir = golden(&program, "find_badge");
    let manifest = Manifest {
        provider: "external".to_owned(),
        backend: Some("my-backend".to_owned()),
        ..manifest(&program, &ir)
    };
    let store = Store::new(&project("external_backend"));
    let id = store.put(&manifest, &ir).expect("stored");
    assert!(read(&store.path(id)).contains(r#""backend":"my-backend""#));
    assert_eq!(store.get(id).expect("loads").manifest, manifest);
}

/// A file that exists but can't be read is a file error with its cause, not a missing artifact: rebuilding would not
/// replace it (R-ART-09).
#[test]
fn an_unreadable_artifact_is_a_file_error() {
    let store = Store::new(&project("unreadable"));
    let id = Fingerprint::of_bytes(b"a directory");
    fs::create_dir_all(store.path(id)).expect("directory in the artifact's place");
    let error = store.get(id).expect_err("unreadable");
    assert!(matches!(error, LoadError::Unreadable { .. }), "{error:?}");
    let diag = error.diagnostic("FindBadge", Span::default());
    assert_eq!(diag.code, Code::FileError);
    assert!(
        diag.message.starts_with("I couldn't open `") && !diag.message.contains("velme build"),
        "{diag:?}"
    );
    assert!(diag.notes.iter().any(|n| n.contains("`FindBadge`")), "{diag:?}");
}
