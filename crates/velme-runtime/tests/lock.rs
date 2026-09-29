//! `velme.lock` and loading a locked goal (`runtime/32` §5, R-ART-10, R-ART-14), with IR installed by the fixture
//! installer (D-16).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use velme_builtins::BUILTINS_VERSION;
use velme_diagnostics::{Code, Span};
use velme_ir::{Fingerprint, IR_VERSION, calls, to_canonical_string};
use velme_runtime::{
    Artifact, Cause, Entry, EntryError, LOCK_FILE, LoadError, Lock, LockError, LockedGoal, MAX_LOCK_BYTES,
    RecordChange, Store, Versioned, load,
};
use velme_sema::hir::Program;
use velme_test_support::{fixture_manifest, goal_id, install, install_artifact, program, read, valid_ir};

/// The project file every goal below is declared in.
const FILE: &str = "game.velme";

const SOURCE: &str = "language: velme/0.1\n\n\
type Player:\n    name: Text\n    score: Number\n\n\
goal Score(player: Player) -> Number:\n    plan: \"Return the player's score.\"\n\n\
goal Double(n: Number) -> Number:\n    plan: \"Double it.\"\n\n\
goal Report(player: Player) -> Number:\n    call:\n        score = Score(player)\n        twice = Double(score)\n    \
plan: \"Report the doubled score.\"\n";

/// An empty project directory for one test.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lock").join(name);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

/// Hand-written IR for a goal of [`SOURCE`], with the compiler's calls.
fn ir(program: &Program, goal: &str) -> String {
    let player = json!({"Player": {"fields": [["name", {"t": "Text"}], ["score", {"t": "Number"}]]}});
    let record = json!({"t": "Record", "name": "Player"});
    let number = json!({"t": "Number"});
    let (types, inputs, body) = match goal {
        "Score" => (
            player,
            json!([["player", record]]),
            json!({"kind": "field", "of": {"kind": "input", "name": "player"}, "field": "score"}),
        ),
        "Double" => (
            json!({}),
            json!([["n", number]]),
            json!({"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                   "right": {"kind": "literal", "type": number, "value": 2}}),
        ),
        "Report" => (
            player,
            json!([["player", record]]),
            json!({"kind": "local", "name": "twice"}),
        ),
        _ => panic!("no goal {goal}"),
    };
    let calls = calls(program, goal_id(program, goal)).expect("calls");
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal, "types": types,
           "inputs": inputs, "output": number, "calls": calls, "body": body})
    .to_string()
}

/// A project with every goal of [`SOURCE`] installed.
fn installed(name: &str) -> (PathBuf, Program) {
    let project = project(name);
    let program = program(SOURCE);
    for goal in ["Score", "Double", "Report"] {
        install(&project, FILE, &program, &ir(&program, goal));
    }
    (project, program)
}

fn lock(project: &Path) -> Lock {
    Lock::read(project).expect("readable").expect("a lock")
}

fn load_goal(project: &Path, program: &Program, goal: &str) -> Result<LockedGoal, EntryError> {
    load(
        program,
        goal_id(program, goal),
        FILE,
        &lock(project),
        &Store::new(project),
    )
}

/// The causes `goal` is stale with.
fn stale(project: &Path, program: &Program, goal: &str) -> Vec<Cause> {
    match load_goal(project, program, goal) {
        Err(EntryError::Stale(causes)) => causes,
        other => panic!("{goal}: {other:?}"),
    }
}

fn record(name: &str, change: RecordChange) -> Cause {
    Cause::Record {
        name: name.to_owned(),
        change,
    }
}

// ---- criteria ----

#[test]
fn ac_art_04_adding_a_field_to_player_makes_every_goal_reaching_it_stale() {
    let (project, before) = installed("ac_art_04");
    for goal in ["Score", "Double", "Report"] {
        load_goal(&project, &before, goal).unwrap_or_else(|e| panic!("{goal}: {e}"));
    }
    let after = program(&SOURCE.replace("    score: Number\n", "    score: Number\n    level: Number\n"));
    let gained = record("Player", RecordChange::Added("level".to_owned()));
    assert_eq!(stale(&project, &after, "Score"), std::slice::from_ref(&gained));
    let score = Cause::Child {
        goal: "Score".to_owned(),
    };
    assert_eq!(stale(&project, &after, "Report"), [gained.clone(), score]);
    // Its signature doesn't reach `Player` (R-ART-02).
    load_goal(&project, &after, "Double").expect("fresh");
    let diag = EntryError::Stale(vec![gained]).diagnostic("Score", Span::default());
    assert_eq!(diag.code, Code::LockStale);
    assert_eq!(
        diag.message,
        "`Score` changed since it was last built — run `velme build`."
    );
    assert_eq!(diag.notes, ["`Player` gained a field `level`"]);
}

/// The file itself is intact; only the lock and the manifest disagree (D-46).
#[test]
fn ac_art_12_manifest_contract_key_differing_from_the_lock_is_stale() {
    let project = project("ac_art_12");
    let program = program(SOURCE);
    let valid = valid_ir(&program, &ir(&program, "Double"));
    let mut manifest = fixture_manifest(&program, &valid);
    manifest.contract_key = Fingerprint::of_bytes(b"another contract");
    let artifact = install_artifact(&project, FILE, &program, &manifest, &valid);
    let store = Store::new(&project);
    assert_eq!(store.get(artifact).expect("its hash matches").manifest, manifest);
    let causes = stale(&project, &program, "Double");
    assert_eq!(causes, [Cause::Manifest { field: "contract_key" }]);
    let diag = EntryError::Stale(causes).diagnostic("Double", Span::default());
    assert_eq!(diag.code, Code::LockStale);
    assert_eq!(
        diag.notes,
        ["the stored artifact doesn't match its own lock entry: its `contract_key` differs"]
    );
}

#[test]
fn ac_art_06_a_locked_goal_whose_artifact_is_tampered_or_deleted_fails_to_load() {
    let (project, program) = installed("ac_art_06");
    let path = Store::new(&project).path(lock(&project).entry(FILE, "Double").expect("entry").artifact);
    let text = read(&path);
    fs::write(&path, text.replacen("\"examples\":0", "\"examples\":1", 1)).expect("tampered");
    let error = load_goal(&project, &program, "Double").expect_err("tampered");
    assert!(
        matches!(error, EntryError::Load(LoadError::Corrupt { .. })),
        "{error:?}"
    );
    assert_eq!(error.code(), Code::ArtifactCorrupt);
    fs::remove_file(&path).expect("deleted");
    let error = load_goal(&project, &program, "Double").expect_err("deleted");
    assert_eq!(error.code(), Code::ArtifactUnavailable);
    assert_eq!(
        error.diagnostic("Double", Span::default()).message,
        "The built version of `Double` is missing — run `velme build`."
    );
}

// ---- loading (R-ART-10, R-ART-14) ----

#[test]
fn a_fresh_entry_loads_its_validated_ir() {
    let (project, program) = installed("fresh");
    let locked = load_goal(&project, &program, "Report").expect("fresh");
    assert_eq!(locked.ir, valid_ir(&program, &ir(&program, "Report")));
    assert_eq!(
        locked.artifact,
        lock(&project).entry(FILE, "Report").expect("entry").artifact
    );
    assert_eq!(locked.manifest.goal, "Report");
}

#[test]
fn causes_name_what_changed() {
    let (project, built) = installed("causes");
    let plan = program(&SOURCE.replace("Return the player's score.", "Return the score."));
    assert_eq!(stale(&project, &plan, "Score"), [Cause::Source]);
    // Only `Score`'s own key changed (D-11).
    load_goal(&project, &plan, "Report").expect("fresh");
    let renamed = program(&SOURCE.replace("Double(n: Number)", "Double(x: Number)"));
    assert_eq!(stale(&project, &renamed, "Double"), [Cause::Inputs]);
    let double = Cause::Child {
        goal: "Double".to_owned(),
    };
    assert_eq!(stale(&project, &renamed, "Report"), [double]);
    let retyped = program(&SOURCE.replace("    name: Text\n", "    name: Number\n"));
    assert_eq!(
        stale(&project, &retyped, "Score"),
        [record("Player", RecordChange::Retyped("name".to_owned()))]
    );
    let rewired = program(&SOURCE.replace("twice = Double(score)", "twice = Double(4)"));
    assert_eq!(stale(&project, &rewired, "Report"), [Cause::Calls]);
    let extra = program(&format!(
        "{SOURCE}\ngoal Nowhere(n: Number) -> Number:\n    plan: \"Test.\"\n"
    ));
    assert_eq!(stale(&project, &extra, "Nowhere"), [Cause::NotLocked]);
    load_goal(&project, &built, "Score").expect("fresh");
}

/// An entry that disagrees with an artifact built from the current source names the entry, not a source change; an
/// entry pinning another goal's artifact says so rather than diffing that goal (D-46).
#[test]
fn an_edited_lock_entry_is_named_as_such() {
    let (project, program) = installed("edited_entry");
    let mut lock = lock(&project);
    let double = lock.entry(FILE, "Double").expect("entry").clone();
    let score = lock.entry(FILE, "Score").expect("entry").clone();
    lock.insert(Entry {
        contract_key: Fingerprint::of_bytes(b"edited"),
        ..double.clone()
    });
    lock.insert(Entry {
        artifact: double.artifact,
        ..score
    });
    lock.write(&project).expect("lock written");
    assert_eq!(
        stale(&project, &program, "Double"),
        [Cause::Manifest { field: "contract_key" }]
    );
    assert_eq!(stale(&project, &program, "Score"), [Cause::Manifest { field: "goal" }]);
}

/// A child called twice whose signature changed is named once.
#[test]
fn a_child_called_twice_is_named_once() {
    let project = project("twice");
    let source = SOURCE.replace(
        "        twice = Double(score)\n",
        "        twice = Double(score)\n        again = Double(twice)\n",
    );
    let built = program(&source);
    let mut report: serde_json::Value = serde_json::from_str(&ir(&built, "Report")).expect("json");
    report["body"] = json!({"kind": "local", "name": "again"});
    install(&project, FILE, &built, &report.to_string());
    let renamed = program(&source.replace("Double(n: Number)", "Double(x: Number)"));
    let double = Cause::Child {
        goal: "Double".to_owned(),
    };
    assert_eq!(stale(&project, &renamed, "Report"), [double]);
}

/// A goal whose source changed is stale whether or not its old artifact is still there; only a current entry's
/// artifact must load.
#[test]
fn a_changed_goal_is_stale_even_without_its_artifact() {
    let (project, built) = installed("no_store");
    fs::remove_dir_all(project.join(".velme")).expect("store deleted");
    let edited = program(&SOURCE.replace("Double it.", "Twice it."));
    assert_eq!(stale(&project, &edited, "Double"), [Cause::Source]);
    let error = load_goal(&project, &built, "Double").expect_err("no artifact");
    assert_eq!(error.code(), Code::ArtifactUnavailable);
}

/// A stale entry built for another language version says so (R-ART-14).
#[test]
fn a_stale_entry_names_the_version_it_was_built_for() {
    let project = project("version");
    let built = program(SOURCE);
    let valid = valid_ir(&built, &ir(&built, "Double"));
    let mut manifest = fixture_manifest(&built, &valid);
    manifest.language_version = "0.0".to_owned();
    install_artifact(&project, FILE, &built, &manifest, &valid);
    let edited = program(&SOURCE.replace("Double it.", "Twice it."));
    let causes = stale(&project, &edited, "Double");
    assert_eq!(
        causes,
        [Cause::Version {
            of: Versioned::Language,
            built: "0.0".to_owned(),
            now: "0.1".to_owned(),
        }]
    );
    assert_eq!(causes[0].to_string(), "built for language 0.0, the file says 0.1");
}

#[test]
fn stored_ir_that_no_longer_validates_is_stale() {
    let (project, program) = installed("revalidated");
    let store = Store::new(&project);
    let mut lock = lock(&project);
    let entry = lock.entry(FILE, "Double").expect("entry").clone();
    let mut artifact: Artifact = store.get(entry.artifact).expect("stored");
    artifact.ir.body =
        serde_json::from_value(json!({"kind": "literal", "type": {"t": "Text"}, "value": "two"})).expect("a node");
    // A hash-consistent file and lock entry, as an edited project could hold (R-ART-10).
    let text = to_canonical_string(&artifact).expect("canonical");
    let id = Fingerprint::of_bytes(text.as_bytes());
    fs::write(store.path(id), text).expect("written");
    lock.insert(Entry { artifact: id, ..entry });
    lock.write(&project).expect("lock written");
    let causes = stale(&project, &program, "Double");
    let [Cause::Invalid(diags)] = causes.as_slice() else {
        panic!("{causes:?}")
    };
    assert_eq!(diags[0].code, Code::IRInvalid);
    assert!(
        causes[0]
            .to_string()
            .starts_with("the stored artifact is no longer valid: ")
    );
}

// ---- the lock file (R-ART-13) ----

fn entry(file: &str, name: &str) -> Entry {
    Entry {
        file: file.to_owned(),
        name: name.to_owned(),
        signature: Fingerprint::of_bytes(format!("{name} signature").as_bytes()),
        contract_key: Fingerprint::of_bytes(format!("{name} contract").as_bytes()),
        artifact: Fingerprint::of_bytes(format!("{name} artifact").as_bytes()),
    }
}

#[test]
fn lock_entries_are_sorted_and_written_byte_deterministically() {
    let mut lock = Lock::new("0.1");
    lock.insert(entry("player.velme", "FindBadge"));
    lock.insert(entry("dir/\"quoted\".velme", "Zed"));
    lock.insert(entry("player.velme", "BuildPlayerSummary"));
    // Replacing an entry keeps one per goal.
    lock.insert(entry("player.velme", "FindBadge"));
    insta::assert_snapshot!("lock_file", lock.to_text());
    assert_eq!(Lock::parse(&lock.to_text()).expect("reads back"), lock);
    let project = project("deterministic");
    lock.write(&project).expect("written");
    let first = fs::read(project.join(LOCK_FILE)).expect("lock");
    lock.write(&project).expect("written again");
    assert_eq!(fs::read(project.join(LOCK_FILE)).expect("lock"), first);
}

/// Installing the same fixtures again leaves the lock and the store byte-identical (R-ART-13, R-ART-09).
#[test]
fn reinstalling_fixtures_changes_nothing() {
    let (project, program) = installed("reinstall");
    let snapshot = |project: &Path| {
        let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(Store::new(project).dir())
            .expect("store")
            .map(|e| {
                let e = e.expect("entry");
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read(e.path()).expect("file"),
                )
            })
            .collect();
        files.sort();
        (fs::read(project.join(LOCK_FILE)).expect("lock"), files)
    };
    let before = snapshot(&project);
    for goal in ["Report", "Double", "Score"] {
        install(&project, FILE, &program, &ir(&program, goal));
    }
    assert_eq!(snapshot(&project), before);
    assert_eq!(before.1.len(), 3);
}

#[test]
fn a_lock_velme_cannot_read_is_a_file_error() {
    let partial = "[[goal]]\nfile = \"a.velme\"\nname = \"A\"\nsignature = \"b3:00\"\n";
    for (text, reason) in [
        // The format is named even when the newer lock has fields this one doesn't.
        (
            "version = 2\nlanguage = \"0.1\"\nextra = true\n",
            "it is lock format 2, but this version of Velme reads format 1",
        ),
        (
            "version = 1\nlanguage = \"0.1\"\nextra = true\n",
            "unknown field `extra`",
        ),
        (&format!("version = 1\nlanguage = \"0.1\"\n{partial}"), "not `b3:`"),
    ] {
        let error = Lock::parse(text).expect_err(text);
        let diag = error.diagnostic();
        assert_eq!(diag.code, Code::FileError, "{text}");
        assert_eq!(diag.message, "I couldn't open `velme.lock`.");
        assert!(diag.notes[0].contains(reason), "{text}: {diag:#?}");
    }
    let newer = Lock::parse("version = 2\nlanguage = \"0.1\"\n")
        .expect_err("newer")
        .diagnostic();
    assert_eq!(
        newer.help.as_deref(),
        Some("it was written by a newer Velme: upgrade Velme to use it")
    );
    let older = Lock::parse("version = 0\nlanguage = \"0.1\"\n")
        .expect_err("older")
        .diagnostic();
    assert_eq!(
        older.help.as_deref(),
        Some("`velme build` writes `velme.lock`; restore it from version control")
    );
    let one = entry_text("A");
    let error = Lock::parse(&format!("version = 1\nlanguage = \"0.1\"\n{one}{one}")).expect_err("twice");
    assert_eq!(error.diagnostic().notes, ["it pins the goal `A` twice"]);
    assert_eq!(Lock::read(&project("no_lock")).expect("no error"), None);
}

/// A lock is a regular file of at most `MAX_LOCK_BYTES`, and is written only through Velme's own `.velme/tmp`.
#[test]
fn a_lock_must_be_a_regular_file_of_bounded_size() {
    let root = project("long_lock");
    fs::write(
        Lock::path(&root),
        vec![b'#'; usize::try_from(MAX_LOCK_BYTES).expect("fits") + 1],
    )
    .expect("written");
    let error = Lock::read(&root).expect_err("too long");
    assert!(error.to_string().contains("longer than"), "{error}");
    assert_eq!(error.code(), Code::FileError);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let root = project("linked_lock");
        let target = root.join("elsewhere.lock");
        Lock::new("0.1").write(&root).expect("written");
        fs::rename(Lock::path(&root), &target).expect("moved");
        symlink(&target, Lock::path(&root)).expect("linked");
        let error = Lock::read(&root).expect_err("a link");
        assert!(matches!(error, LockError::Unreadable { .. }), "{error:?}");

        let root = project("linked_tmp");
        fs::create_dir_all(root.join(".velme")).expect(".velme");
        fs::create_dir_all(root.join("target")).expect("target");
        symlink(root.join("target"), root.join(".velme/tmp")).expect("linked");
        let error = Lock::new("0.1").write(&root).expect_err("refused");
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }
}

/// The lock text of one entry.
fn entry_text(name: &str) -> String {
    let mut lock = Lock::new("0.1");
    lock.insert(entry("a.velme", name));
    let text = lock.to_text();
    let at = text.find("[[goal]]").expect("an entry");
    text[at..].to_owned()
}
