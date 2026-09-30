//! The artifact document and the local store (`runtime/32` §3–4).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use velme_builtins::BUILTINS_VERSION;
use velme_diagnostics::{Code, Span};
use velme_ir::{
    CallNode, Fingerprint, IR_VERSION, MAX_JSON_DEPTH, Synthesis, ValidIr, contract_key, from_json_str,
    from_json_str_within, signature, synthesis_key, to_canonical_string,
};
use velme_runtime::{
    ARTIFACTS_DIR, Artifact, ArtifactFormat, Child, Kind, LoadError, MAX_ARTIFACT_BYTES, Manifest, Store, StoreError,
    TMP_DIR, VELME_DIR, Verification,
};
use velme_sema::hir::Program;
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
        format: ArtifactFormat,
        goal: goal.goal.clone(),
        kind: program.goals[id.0].kind.into(),
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
        "format",
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
    // R-ART-09: an artifact already stored is left as it is.
    let stored = fs::metadata(store.path(leaf_id))
        .expect("stored")
        .modified()
        .expect("mtime");
    assert_eq!(store.put(&manifest(&program, &leaf), &leaf).expect("stored"), leaf_id);
    let again = fs::metadata(store.path(leaf_id))
        .expect("stored")
        .modified()
        .expect("mtime");
    assert_eq!(stored, again);
    // A file under the name with other bytes isn't that artifact: it is replaced whole.
    fs::write(store.path(leaf_id), "damaged").expect("damaged");
    assert_eq!(store.put(&manifest(&program, &leaf), &leaf).expect("stored"), leaf_id);
    let bytes = fs::read(store.path(leaf_id)).expect("replaced");
    assert_eq!(Fingerprint::of_bytes(&bytes), leaf_id);
    assert_eq!(files(store.dir()), names);
    assert_eq!(files(&tmp), Vec::<String>::new());
}

/// R-ART-10: the store writes only canonical JSON of at most `MAX_ARTIFACT_BYTES`, so a file that hashes to its name
/// but is laid out otherwise, or is longer, was not written by it: `VL0703`.
#[test]
fn ac_art_06_an_artifact_must_be_canonical_and_of_bounded_size() {
    let (store, id, artifact) = stored("canonical");
    let pretty =
        serde_json::to_string_pretty(&from_json_str::<Value>(&read(&store.path(id))).expect("JSON")).expect("pretty");
    let other = Fingerprint::of_bytes(pretty.as_bytes());
    fs::write(store.path(other), &pretty).expect("written");
    assert_eq!(
        from_json_str::<Artifact>(&pretty).expect("an artifact document"),
        artifact
    );
    let error = store.get(other).expect_err("not canonical");
    assert!(matches!(error, LoadError::Damaged { .. }), "{error:?}");
    let diag = error.diagnostic("BuildPlayerSummary", Span::default());
    assert_eq!(diag.code, Code::ArtifactCorrupt);
    assert!(diag.notes.iter().any(|n| n.contains("canonical")), "{diag:?}");

    let long = vec![b' '; usize::try_from(MAX_ARTIFACT_BYTES).expect("fits") + 1];
    let long_id = Fingerprint::of_bytes(&long);
    fs::write(store.path(long_id), &long).expect("written");
    let error = store.get(long_id).expect_err("too long");
    assert!(matches!(error, LoadError::Damaged { .. }), "{error:?}");
    assert_eq!(error.code(), Code::ArtifactCorrupt);
}

/// An artifact whose canonical JSON is longer than any `get` reads is not stored.
#[test]
fn an_oversized_artifact_is_not_stored() {
    let program = goals();
    let ir = golden(&program, "find_badge");
    let manifest = Manifest {
        model_version: Some("m".repeat(usize::try_from(MAX_ARTIFACT_BYTES).expect("fits"))),
        ..manifest(&program, &ir)
    };
    let store = Store::new(&project("oversized"));
    let error = store.put(&manifest, &ir).expect_err("too large");
    assert!(matches!(error, StoreError::TooLarge { .. }), "{error:?}");
    assert!(!store.dir().exists());
}

/// D-86: an artifact of another format, or of none, is not read, as a document that is no artifact isn't.
#[test]
fn an_artifact_of_another_format_is_not_read() {
    let (store, id, _) = stored("format");
    let document: Value = from_json_str(&read(&store.path(id))).expect("JSON");
    for format in [Some("velme-artifact/2"), None] {
        let mut edited = document.clone();
        let manifest = edited["manifest"].as_object_mut().expect("manifest");
        match format {
            Some(format) => manifest.insert("format".to_owned(), Value::from(format)),
            None => manifest.remove("format"),
        };
        let text = to_canonical_string(&edited).expect("canonical");
        let other = Fingerprint::of_bytes(text.as_bytes());
        fs::write(store.path(other), &text).expect("written");
        let error = store.get(other).expect_err("another format");
        assert!(matches!(error, LoadError::Malformed { .. }), "{error:?}");
        assert_eq!(error.code(), Code::LockStale);
        assert!(error.to_string().contains("format"), "{error}");
    }
}

/// Velme reads and writes only regular files in its own `.velme` directories, never through a link (R-ART-09).
#[cfg(unix)]
#[test]
fn links_in_the_store_are_refused() {
    use std::os::unix::fs::symlink;

    let (store, id, _) = stored("links_file");
    let path = store.path(id);
    let elsewhere = path.with_extension("elsewhere");
    fs::rename(&path, &elsewhere).expect("moved");
    symlink(&elsewhere, &path).expect("linked");
    let error = store.get(id).expect_err("a link");
    assert!(matches!(error, LoadError::Unreadable { .. }), "{error:?}");
    assert_eq!(error.code(), Code::FileError);
    // Storing the artifact doesn't replace what isn't a regular file (R-ART-09).
    let program = goals();
    let ir = golden(&program, "player_summary");
    let error = store
        .put(&manifest(&program, &ir), &ir)
        .expect_err("a link in its place");
    assert!(matches!(error, StoreError::Io(_)), "{error:?}");
    assert!(
        fs::symlink_metadata(&path)
            .expect("still there")
            .file_type()
            .is_symlink()
    );
    // A FIFO is refused without blocking the read.
    fs::remove_file(&path).expect("unlinked");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let error = store.get(id).expect_err("a FIFO");
    assert!(matches!(error, LoadError::Unreadable { .. }), "{error:?}");
    assert!(matches!(
        store.put(&manifest(&program, &ir), &ir),
        Err(StoreError::Io(_))
    ));

    // A `.velme` or `.velme/tmp` that is a link is refused for reading and writing.
    let ir = golden(&program, "find_badge");
    for linked in [VELME_DIR, "tmp"] {
        let root = project(&format!("links_{linked}"));
        let target = root.join("target");
        fs::create_dir_all(&target).expect("target");
        let velme = root.join(VELME_DIR);
        let link = if linked == VELME_DIR {
            velme.clone()
        } else {
            fs::create_dir_all(&velme).expect(".velme");
            velme.join(TMP_DIR)
        };
        symlink(&target, &link).expect("linked");
        let store = Store::new(&root);
        let error = store.put(&manifest(&program, &ir), &ir).expect_err("refused");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(files(&target), Vec::<String>::new());
        if linked == VELME_DIR {
            let error = store.get(id).expect_err("refused");
            assert!(matches!(error, LoadError::Unreadable { .. }), "{error:?}");
        }
    }
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
        kind: Kind::Wired,
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

/// A file that exists but can't be read, such as a directory, is a file error with its cause, not a missing artifact.
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

/// R-ART-10: IR nested as deep as `MAX_JSON_DEPTH` allows stores and loads, its artifact one level deeper; an artifact
/// nested past that is not read. It runs on 8 MiB: a debug build takes about 1.4 MiB to build, compare and drop values
/// this deep, too near a default test thread's 2 MiB to hold on every platform.
#[test]
fn ir_at_the_depth_limit_stores_and_loads() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(stores_and_loads_ir_at_the_depth_limit)
        .expect("thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

fn stores_and_loads_ir_at_the_depth_limit() {
    let program = program("language: velme/0.1\n\ngoal Words(xs: List<Number>) -> Text:\n    plan: \"Test.\"\n");
    // The literal's type starts 4 levels into the IR, under `body`, `cond` and `left`, and ends at the limit.
    let depth = MAX_JSON_DEPTH - 5;
    let ty = (0..depth).fold(r#"{"t": "Number"}"#.to_owned(), |of, _| {
        format!(r#"{{"t": "List", "of": {of}}}"#)
    });
    let value = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
    let literal = format!(r#"{{"kind": "literal", "type": {ty}, "value": {value}}}"#);
    let text = |t: &str| format!(r#"{{"kind": "literal", "type": {{"t": "Text"}}, "value": "{t}"}}"#);
    let ir = format!(
        r#"{{"ir_version": "{IR_VERSION}", "builtins_version": "{BUILTINS_VERSION}", "goal": "Words", "types": {{}},
            "inputs": [["xs", {{"t": "List", "of": {{"t": "Number"}}}}]], "output": {{"t": "Text"}},
            "body": {{"kind": "if", "cond": {{"kind": "binary", "op": "eq", "left": {literal}, "right": {literal}}},
                "then": {}, "else": {}}}}}"#,
        text("same"),
        text("different")
    );
    assert!(from_json_str::<Value>(&ir).is_ok());
    assert!(
        from_json_str_within::<Value>(&ir, MAX_JSON_DEPTH - 1).is_err(),
        "not at the limit"
    );
    let ir = valid_ir(&program, &ir);
    let store = Store::new(&project("depth"));
    let id = store.put(&manifest(&program, &ir), &ir).expect("stored");
    assert_eq!(store.get(id).expect("loads").ir, *ir.goal());

    // One level more, under the literal's type.
    let mut document: Value = from_json_str_within(&read(&store.path(id)), MAX_JSON_DEPTH + 1).expect("JSON");
    let ty = document
        .pointer_mut("/ir/body/cond/left/type")
        .expect("the literal's type");
    *ty = serde_json::json!({"t": "List", "of": ty.take()});
    let deeper = to_canonical_string(&document).expect("canonical");
    let deeper_id = Fingerprint::of_bytes(deeper.as_bytes());
    fs::write(store.path(deeper_id), &deeper).expect("written");
    let error = store.get(deeper_id).expect_err("too deep");
    assert!(matches!(error, LoadError::Malformed { .. }), "{error:?}");
    assert!(
        error
            .to_string()
            .contains(&format!("deeper than {}", MAX_JSON_DEPTH + 1)),
        "{error}"
    );
}

/// `collect` deletes an unreferenced store or temporary file only once it is old enough, and only when it is named as the
/// store names its files, in lowercase hex (R-CLI-23, D-111). `now` and the age are injected, so no test waits or reads a
/// clock.
#[test]
fn collect_keeps_recent_files_and_names_that_are_not_lowercase_hex() {
    use std::time::{Duration, SystemTime};
    let (store, id, _) = stored("collect_age");
    let root = project("collect_age_root");
    let store_dir = root.join(VELME_DIR).join(ARTIFACTS_DIR);
    let tmp_dir = root.join(VELME_DIR).join(TMP_DIR);
    fs::create_dir_all(&store_dir).expect("store");
    fs::create_dir_all(&tmp_dir).expect("tmp");
    let store_files = Store::new(&root);
    let stray = store_dir.join(format!("b3-{}.json", "0".repeat(64)));
    let upper = store_dir.join(format!("b3-{}.json", "A".repeat(64)));
    let temp = tmp_dir.join("left-over");
    for path in [&stray, &upper, &temp] {
        fs::write(path, b"{}").expect("file");
    }
    let age = Duration::from_secs(600);
    let now = SystemTime::now();
    // Just written: nothing is old enough. A `now` in the past makes the files "from the future", also kept.
    assert_eq!(store_files.collect(&[], now, age).expect("collected"), 0);
    assert_eq!(
        store_files
            .collect(&[], now - Duration::from_secs(3600), age)
            .expect("collected"),
        0
    );
    assert!(stray.is_file() && upper.is_file() && temp.is_file());
    // Ten minutes and a second on: the stray and the temporary file go; the upper-case name was never a store file.
    let later = now + age + Duration::from_secs(1);
    assert_eq!(store_files.collect(&[], later, age).expect("collected"), 2);
    assert!(!stray.exists() && !temp.exists() && upper.is_file());
    // A referenced artifact stays however old.
    assert_eq!(store.collect(&[id], later, Duration::ZERO).expect("collected"), 0);
    assert!(store.path(id).is_file());
}
