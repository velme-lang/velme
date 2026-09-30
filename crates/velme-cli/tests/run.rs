//! `velme run`, `velme test` and `velme check`'s locked IR on the committed fixture projects (`tooling/40` §2–4),
//! whose artifacts are hand-written IR installed by the fixture installer (D-16).
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use velme_runtime::{ARTIFACTS_DIR, LOCK_FILE, VELME_DIR};
use velme_test_support::differential::EXAMPLES;
use velme_test_support::{goal_id, install, program, read, repo};

/// Set to rewrite the committed fixture projects from their hand-written IR, then review the diff.
const BLESS: &str = "VELME_BLESS_FIXTURES";

/// The single-goal fixture projects: each is `examples/beginner/add.velme` with the IR of
/// `tests/fixtures/run/<name>.json`.
const FIXTURES: [&str; 2] = ["add", "add_broken"];

/// The fixture projects made from an example of the launch demos, with the IR of each of its goals in
/// `tests/fixtures/run/<name>.ir/<Goal>.json`: each fixture's name and its example's path, from the one list of
/// examples.
fn examples() -> impl Iterator<Item = (&'static str, &'static str)> {
    EXAMPLES.iter().filter_map(|(example, ir)| {
        let name = ir.strip_prefix("tests/fixtures/run/")?.strip_suffix(".ir")?;
        Some((name, *example))
    })
}

/// The project configuration file (`tooling/40` §5.1); the binary's own constant isn't reachable from a test.
const CONFIG_FILE: &str = "velme.toml";

const EXAMPLE: &str = "examples/beginner/add.velme";

const GOOD: &str = "tests/fixtures/run/add/add.velme";
const BROKEN: &str = "tests/fixtures/run/add_broken/add.velme";

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Runs `velme` from the repository root.
fn velme(args: &[&str]) -> Run {
    velme_in(&repo(""), args, b"")
}

/// Runs `velme` in `dir` with `stdin` as its standard input.
fn velme_in(dir: &Path, args: &[&str], stdin: &[u8]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_velme"))
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("velme runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin)
        .expect("stdin written");
    let out = child.wait_with_output().expect("velme exits");
    Run {
        stdout: String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is UTF-8"),
        code: out.status.code().expect("velme exits with a code"),
    }
}

/// Every file under `dir` with its bytes, by path from `dir`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .expect("inside")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(name, fs::read(&path).expect("file"));
            }
        }
    }
    files
}

/// The fixture project `name` as the installer makes it, in a fresh directory: the source, the lock and the store.
fn made(name: &str) -> PathBuf {
    let dir = scratch(&format!("made_{name}"));
    let source = read(&repo(EXAMPLE));
    fs::write(dir.join("add.velme"), &source).expect("source");
    install(
        &dir,
        "add.velme",
        &program(&source),
        &read(&repo(&format!("tests/fixtures/run/{name}.json"))),
    );
    // The installer's temporary directory is not part of a project (`runtime/32` §4).
    fs::remove_dir_all(dir.join(VELME_DIR).join("tmp")).expect("temporary directory");
    dir
}

/// The fixture project `name` made from its example as the installer makes it: every goal's hand-written IR, with the
/// compiler's `calls` joined in as a synthesized reply's are (`compiler/21` R-IR-02), so a fixture doesn't spell out
/// signature fingerprints.
fn made_example(name: &str, example: &str) -> PathBuf {
    let dir = scratch(&format!("made_{name}"));
    let source = read(&repo(example));
    let file = Path::new(example)
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .into_owned();
    fs::write(dir.join(&file), &source).expect("source");
    let program = program(&source);
    let mut irs: Vec<PathBuf> = fs::read_dir(repo(&format!("tests/fixtures/run/{name}.ir")))
        .expect("the IR directory")
        .map(|e| e.expect("entry").path())
        .collect();
    irs.sort();
    for path in irs {
        let mut ir: Value = serde_json::from_str(&read(&path)).expect("IR JSON");
        let goal = ir["goal"].as_str().expect("a goal name").to_owned();
        let calls = velme_ir::calls(&program, goal_id(&program, &goal)).expect("calls");
        ir["calls"] = serde_json::to_value(calls).expect("calls as JSON");
        install(&dir, &file, &program, &ir.to_string());
    }
    fs::remove_dir_all(dir.join(VELME_DIR).join("tmp")).expect("temporary directory");
    dir
}

/// Held while the committed fixtures are rewritten (R-QA-09), and by whatever reads them meanwhile: tests run in parallel.
static FIXTURES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn fixtures_lock() -> std::sync::MutexGuard<'static, ()> {
    FIXTURES_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An empty directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("run").join(name);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// A copy of the fixture project `name` to change.
fn copy(name: &str, test: &str) -> PathBuf {
    let dir = scratch(test);
    let _fixtures = fixtures_lock();
    for (file, bytes) in tree(&repo(&format!("tests/fixtures/run/{name}"))) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        fs::write(path, bytes).expect("copied");
    }
    dir
}

fn json(run: &Run) -> Value {
    serde_json::from_str(&run.stdout).expect("--json prints one JSON document")
}

/// The committed fixture projects are exactly what the installer makes from their IR (D-16).
#[test]
fn fixture_projects_are_the_installers_output() {
    let projects = FIXTURES
        .iter()
        .map(|name| (*name, made(name), 1))
        .chain(examples().map(|(name, example)| {
            let goals = fs::read_dir(repo(&format!("tests/fixtures/run/{name}.ir")))
                .expect("IR")
                .count();
            (name, made_example(name, example), goals)
        }));
    for (name, project, goals) in projects {
        let committed = repo(&format!("tests/fixtures/run/{name}"));
        let made = tree(&project);
        if std::env::var_os(BLESS).is_some() {
            let _bless = fixtures_lock();
            let _ = fs::remove_dir_all(&committed);
            for (file, bytes) in &made {
                let path = committed.join(file);
                fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
                fs::write(path, bytes).expect("blessed");
            }
        }
        assert_eq!(
            tree(&committed),
            made,
            "{name}: rerun with {BLESS}=1 and review the diff"
        );
        assert!(made.contains_key(LOCK_FILE));
        assert_eq!(
            made.keys()
                .filter(|f| f.starts_with(&format!("{VELME_DIR}/{ARTIFACTS_DIR}/")))
                .count(),
            goals
        );
    }
}

// ---- criteria ----

/// The failed check shows the assertion with its expected and received values (`language/13` §5).
#[test]
fn ac_rdm_06_a_broken_fixture_shows_the_failed_check_with_its_values() {
    let run = velme(&["run", BROKEN, "--goal", "Add", "--arg", "a=2", "--arg", "b=3"]);
    assert_eq!(run.code, 3, "{}", run.stderr);
    assert_eq!(run.stdout, "Add  ✗\n");
    insta::assert_snapshot!(run.stderr);
}

/// `velme check` reads the store only to validate locked IR and writes nothing (`tooling/40` §2). The CLI builds no
/// provider until `velme build` exists (M5b), and `velme-check` has no dependency on `velme-synth` (AC-CMP-01), so
/// `check` cannot reach one. `velme_test_support::PanicProvider` is the panicking provider the build tests pass in.
#[test]
fn ac_cmp_02_check_reads_the_store_only_to_validate_locked_ir() {
    let project = copy("add", "ac_cmp_02");
    let before = tree(&project);
    let file = project.join("add.velme");
    let run = velme(&["check", file.to_str().expect("UTF-8 path")]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        (
            "✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ IR valid        (1 goal locked)\n✓ Checks valid\n",
            "",
            0
        )
    );
    assert_eq!(tree(&project), before);
    // A file with no lock in its project reads no store: the output is phases 1–6 alone.
    let run = velme(&["check", EXAMPLE]);
    assert_eq!(
        run.stdout,
        "✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ Checks valid\n"
    );
}

/// The artifact is checked against its hash on every run (T-4).
#[test]
fn ac_sec_04_a_run_after_editing_one_byte_of_the_artifact_is_corrupt() {
    let project = copy("add", "ac_sec_04");
    let (name, bytes) = tree(&project)
        .into_iter()
        .find(|(f, _)| f.starts_with(VELME_DIR))
        .expect("an artifact");
    let mut edited = bytes;
    let last = edited.len() - 2;
    edited[last] ^= 0x01;
    fs::write(project.join(name), edited).expect("edited");
    let file = project.join("add.velme");
    let file = file.to_str().expect("UTF-8 path");
    let run = velme(&["run", file, "--goal", "Add", "--arg", "a=1", "--arg", "b=2"]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0703]"), "{}", run.stderr);
    let run = velme(&["check", file]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0703]"), "{}", run.stderr);
}

// ---- velme run ----

#[test]
fn run_shows_the_result() {
    let run = velme(&[
        "run",
        GOOD,
        "--goal",
        "Add",
        "--input",
        "tests/fixtures/run/add_input.json",
    ]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        ("Add  ✓\n\nResult:\n5\n", "", 0)
    );
    // `--arg` overrides a key of `--input` (`tooling/40` §3.1).
    let run = velme(&[
        "run",
        GOOD,
        "--goal",
        "Add",
        "--input",
        "tests/fixtures/run/add_input.json",
        "--arg",
        "b=0.25",
        "--json",
    ]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    insta::assert_snapshot!(run.stdout);
}

#[test]
fn run_rejects_missing_extra_and_mistyped_inputs() {
    let run = velme(&["run", GOOD, "--goal", "Add", "--arg", "a=\"two\"", "--arg", "c=1"]);
    assert_eq!(run.code, 64, "{}", run.stderr);
    insta::assert_snapshot!(run.stderr);
}

#[test]
fn run_names_a_goal_that_isnt_there() {
    let run = velme(&["run", GOOD, "--goal", "Ad", "--arg", "a=1", "--arg", "b=2"]);
    assert_eq!(run.code, 64);
    assert!(
        run.stderr
            .contains("There's no goal called `Ad`. Did you mean `Add`?  [VL0903]"),
        "{}",
        run.stderr
    );
}

/// A goal whose source changed since its artifact was built is stale, and nothing runs (R-ART-16).
#[test]
fn run_of_a_changed_goal_is_stale() {
    let project = copy("add", "stale");
    let file = project.join("add.velme");
    let source = read(&file).replace("Add a and b.", "Add both numbers.");
    fs::write(&file, source).expect("edited");
    let run = velme(&[
        "run",
        file.to_str().expect("UTF-8 path"),
        "--goal",
        "Add",
        "--arg",
        "a=1",
        "--arg",
        "b=2",
    ]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("`Add` changed since it was last built — run `velme build`.  [VL0702]"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("its plan, checks, examples or budget changed"),
        "{}",
        run.stderr
    );
    // With a bad input as well, the usage error decides the exit code (R-CLI-16).
    let run = velme(&[
        "run",
        file.to_str().expect("UTF-8 path"),
        "--goal",
        "Add",
        "--arg",
        "a=1",
    ]);
    assert_eq!(run.code, 64, "{}", run.stderr);
}

/// Nothing is synthesized by `velme run`: a goal with calls whose goals aren't built stops the run before it starts,
/// naming each (R-ART-16).
#[test]
fn run_of_a_goal_with_calls_needs_every_goal_built() {
    let run = velme(&[
        "run",
        "examples/beginner/double_then_add_one.velme",
        "--goal",
        "Main",
        "--arg",
        "x=4",
    ]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert_eq!(run.stdout, "Main  ✗\n");
    for goal in ["Main", "Double", "AddOne"] {
        assert!(
            run.stderr
                .contains(&format!("`{goal}` has no verified build in `velme.lock`")),
            "{}",
            run.stderr
        );
    }
}

/// The path of the fixture project `name` made from an example.
fn example_file(name: &str, file: &str) -> String {
    format!("tests/fixtures/run/{name}/{file}")
}

/// `Main` calls `Double`, then `AddOne`, in two waves: `2x + 1` (AC-RUN-02, AC-RDM-02).
#[test]
fn ac_run_02_run_of_a_wired_goal_shows_its_result() {
    let file = example_file("double_then_add_one", "double_then_add_one.velme");
    let run = velme(&["run", &file, "--goal", "Main", "--arg", "x=4"]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        ("Double  ✓\nAddOne  ✓\nMain    ✓\n\nResult:\n9\n", "", 0)
    );
    let run = velme(&["run", &file, "--goal", "Main", "--arg", "x=0.5", "--json"]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let envelope = json(&run);
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["results"][0]["goal"], "Main");
    assert_eq!(envelope["results"][0]["result"].to_string(), "2");
}

/// The launch demos run end to end from hand-written IR: the mixed-wave games example, the sequential order total, the
/// parallel player summary and the beginner leaf.
#[test]
fn the_launch_demos_run_end_to_end() {
    let level = r#"level={"enemy_count":7,"treasure_count":3,"base_score":120}"#;
    let player = r#"player={"name":"Lina","jump_height":3,"score":820}"#;
    let items = r#"items=[{"price":10,"quantity":3},{"price":5,"quantity":1}]"#;
    let cases = [
        ("level_summary", "level_summary", "CreateLevelSummary", level),
        ("order_total", "order_total", "OrderTotal", items),
        ("player_summary", "player_summary", "BuildPlayerSummary", player),
        (
            "find_badge",
            "find_badge",
            "FindBadge",
            r#"player={"name":"Lina","score":820}"#,
        ),
    ];
    for (name, file, goal, arg) in cases {
        let file = example_file(name, &format!("{file}.velme"));
        let run = velme(&["run", &file, "--goal", goal, "--arg", arg, "--json"]);
        assert_eq!(run.code, 0, "{name}: {}{}", run.stdout, run.stderr);
        insta::assert_snapshot!(format!("run_demo_{name}"), run.stdout);
    }
    let file = example_file("order_total", "order_total.velme");
    let big = r#"items=[{"price":100,"quantity":3}]"#;
    let run = velme(&["run", &file, "--goal", "OrderTotal", "--arg", big]);
    assert_eq!(
        run.stdout,
        "Subtotal    ✓\nDiscount    ✓\nShipping    ✓\nOrderTotal  ✓\n\nResult:\n270\n"
    );
}

/// The result is byte-identical for `--jobs 1` and `--jobs 8` on the golden programs (AC-RUN-04, result half; the trace
/// half is `ac_run_04_the_trace_is_the_same_for_every_jobs_value`).
#[test]
fn ac_run_04_the_output_is_the_same_for_every_jobs_value() {
    let level = r#"level={"enemy_count":12,"treasure_count":0,"base_score":9}"#;
    let file = example_file("level_summary", "level_summary.velme");
    let one = velme(&[
        "run",
        &file,
        "--goal",
        "CreateLevelSummary",
        "--arg",
        level,
        "--jobs",
        "1",
        "--json",
    ]);
    let eight = velme(&[
        "run",
        &file,
        "--goal",
        "CreateLevelSummary",
        "--arg",
        level,
        "--jobs",
        "8",
        "--json",
    ]);
    assert_eq!(one.code, 0, "{}", one.stderr);
    assert_eq!(
        (one.stdout, one.stderr, one.code),
        (eight.stdout, eight.stderr, eight.code)
    );
}

/// `--jobs` is a positive whole number, and only `velme run` takes it.
#[test]
fn jobs_must_be_a_positive_number() {
    let file = example_file("double_then_add_one", "double_then_add_one.velme");
    for jobs in ["0", "-1", "many"] {
        let run = velme(&["run", &file, "--goal", "Main", "--arg", "x=4", "--jobs", jobs]);
        assert_eq!(run.code, 64, "{jobs}: {}", run.stderr);
        assert!(run.stderr.starts_with("usage: velme check"), "{}", run.stderr);
    }
    assert_eq!(velme(&["check", &file, "--jobs", "2"]).code, 64);
}

/// Two siblings fail: the exit status and message are the lower source-order binding's, with the other failure a note,
/// and the envelope is the same for every `--jobs` (AC-RUN-03, AC-RUN-04, D-9).
#[test]
fn failed_siblings_report_the_lowest_source_order_failure() {
    let dir = scratch("siblings");
    let source = "language: velme/0.1\n\n\
goal Slow(n: Number) -> Number:\n    plan: \"Count, then divide by zero.\"\n\n\
goal Fast(n: Number) -> Number:\n    plan: \"Divide by zero.\"\n\n\
goal Later(n: Number) -> Number:\n    plan: \"Return n.\"\n\n\
goal Both(n: Number) -> Number:\n    call:\n        first = Slow(n)\n        second = Fast(n)\n        \
third = Later(first)\n    plan: \"Add them.\"\n\n\
goal Top(n: Number) -> Number:\n    call:\n        a = Later(n)\n    plan: \"Return a.\"\n    check:\n        - result < 0\n";
    fs::write(dir.join("both.velme"), source).expect("source");
    let program = program(source);
    let number = serde_json::json!({"t": "Number"});
    let n = serde_json::json!({"kind": "input", "name": "n"});
    let zero = serde_json::json!({"kind": "literal", "type": number, "value": 0});
    let range = serde_json::json!({"kind": "builtin", "name": "range", "args": [n]});
    let count = serde_json::json!({"kind": "reduce", "list": range, "init": zero,
        "fn": {"acc": "a", "param": "i", "body": {"kind": "binary", "op": "add",
               "left": {"kind": "local", "name": "a"}, "right": {"kind": "local", "name": "i"}}}});
    let divide = |left: &Value| serde_json::json!({"kind": "binary", "op": "div", "left": left, "right": zero});
    let bodies = [
        ("Slow", divide(&count)),
        ("Fast", divide(&n)),
        ("Later", n.clone()),
        ("Both", serde_json::json!({"kind": "local", "name": "third"})),
        ("Top", serde_json::json!({"kind": "local", "name": "a"})),
    ];
    for (goal, body) in bodies {
        let calls = velme_ir::calls(&program, goal_id(&program, goal)).expect("calls");
        let ir = serde_json::json!({"ir_version": "0.1", "builtins_version": "0.1", "goal": goal, "types": {},
            "inputs": [["n", number]], "output": number, "calls": calls, "body": body});
        install(&dir, "both.velme", &program, &ir.to_string());
    }
    let file = dir.join("both.velme");
    let file = file.to_str().expect("UTF-8 path");
    let run = |jobs: &str| {
        velme(&[
            "run", file, "--goal", "Both", "--arg", "n=3000", "--jobs", jobs, "--json",
        ])
    };
    let (one, eight) = (run("1"), run("8"));
    assert_eq!(one.code, 3, "{}", one.stderr);
    assert_eq!(
        (&one.stdout, &one.stderr, one.code),
        (&eight.stdout, &eight.stderr, eight.code)
    );
    let envelope = json(&one);
    let diagnostics = envelope["results"][0]["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "VL0602");
    let message = diagnostics[0]["message"].as_str().expect("a message");
    assert!(
        message.starts_with("`Both` failed because `Slow` failed: "),
        "{message}"
    );
    insta::assert_snapshot!(one.stdout);
    // The trace is the same too, and lists what really happened to every call, in source order (AC-RUN-03, AC-RUN-04).
    let traced = |jobs: &str| {
        velme(&[
            "trace", file, "--goal", "Both", "--arg", "n=3000", "--jobs", jobs, "--json",
        ])
    };
    let (one, eight) = (traced("1"), traced("8"));
    assert_eq!(one.code, 3, "{}", one.stderr);
    assert_eq!(without_durations(&json(&one)), without_durations(&json(&eight)));
    let trace = &json(&one)["results"][0]["trace"];
    let calls = trace["goal"]["calls"].as_array().expect("calls");
    let seen: Vec<(&str, &str)> = calls
        .iter()
        .map(|c| {
            (
                c["binding"].as_str().expect("binding"),
                c["outcome"].as_str().expect("outcome"),
            )
        })
        .collect();
    assert_eq!(seen, [("first", "failed"), ("second", "failed"), ("third", "skipped")]);
    assert_eq!(trace["goal"]["failure"]["path"], serde_json::json!(["Both", "Slow"]));
    assert_eq!(trace["goal"]["failure"]["code"], "VL0602");
    // The human trace says what happened first and the code after it (`tooling/40` §3.5).
    let text = velme(&["trace", file, "--goal", "Both", "--arg", "n=3"]);
    assert!(
        text.stdout.contains("no answer.  [VL0602]  (Both › Slow)"),
        "{}",
        text.stdout
    );
    let human = velme(&["run", file, "--goal", "Both", "--arg", "n=3"]);
    assert_eq!(human.code, 3);
    assert_eq!(human.stdout, "Slow   ✗\nFast   ✗\nLater  skipped\nBoth   ✗\n");
    // A failed check after successful calls: each call's value, then the check with what it expected and got.
    let top = velme(&["run", file, "--goal", "Top", "--arg", "n=3"]);
    assert_eq!(top.code, 3, "{}", top.stderr);
    assert_eq!(top.stdout, "Later  ✓ 3\nTop    ✗\n");
    assert!(
        top.stderr.contains("`Top` didn't pass its check: `result < 0`."),
        "{}",
        top.stderr
    );
    assert!(
        human.stderr.contains("`Both` also failed because `Fast` failed"),
        "{}",
        human.stderr
    );
}

// ---- velme test ----

#[test]
fn test_runs_the_examples_of_each_leaf_goal() {
    let run = velme(&["test", GOOD]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        ("Add  ✓ 3 examples, 61 generated inputs\n", "", 0)
    );
    let run = velme(&["test", BROKEN, "--json"]);
    assert_eq!(run.code, 3);
    let envelope = json(&run);
    assert_eq!(envelope["status"], "failed");
    let codes: Vec<&str> = envelope["results"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| d["code"].as_str().expect("a code"))
        .collect();
    assert_eq!(codes, ["VL0502", "VL0501", "VL0502", "VL0501", "VL0502", "VL0501"]);
    let run = velme(&["test", BROKEN]);
    insta::assert_snapshot!(format!("{}{}", run.stdout, run.stderr));
}

// ---- the project (D-82) ----

/// `..` is resolved before the upward search, so a sibling's `velme.toml` isn't taken for the file's project; on a
/// case-insensitive file system any spelling of the name finds its one lock entry (R-CLI-20).
#[test]
fn the_project_is_found_from_the_resolved_path() {
    let parent = scratch("resolved");
    let project = copy("add", "resolved/proj");
    let sibling = parent.join("sibling");
    fs::create_dir_all(&sibling).expect("sibling");
    fs::write(sibling.join(CONFIG_FILE), "").expect("config");
    let run = velme_in(
        &sibling,
        &[
            "run",
            "../proj/add.velme",
            "--goal",
            "Add",
            "--arg",
            "a=1",
            "--arg",
            "b=2",
        ],
        b"",
    );
    assert_eq!(
        (run.stdout.as_str(), run.code),
        ("Add  ✓\n\nResult:\n3\n", 0),
        "{}",
        run.stderr
    );
    if project.join("ADD.velme").exists() {
        let run = velme_in(
            &project,
            &["run", "ADD.velme", "--goal", "Add", "--arg", "a=1", "--arg", "b=2"],
            b"",
        );
        assert_eq!(run.code, 0, "{}", run.stderr);
        assert_eq!(
            fs::read_to_string(project.join(LOCK_FILE))
                .expect("lock")
                .matches("[[goal]]")
                .count(),
            1
        );
    }
}

/// Input that isn't UTF-8 is invalid JSON with no place in the source, from a file or standard input alike.
#[test]
fn input_that_isnt_utf8_has_no_source_place() {
    let dir = scratch("not_utf8");
    fs::write(dir.join("input.json"), b"{\"a\": \xff}").expect("input");
    let file = repo(GOOD);
    let file = file.to_str().expect("UTF-8 path");
    for (input, stdin) in [("input.json", &b""[..]), ("-", &b"{\"a\": \xff}"[..])] {
        let run = velme_in(&dir, &["run", file, "--goal", "Add", "--input", input], stdin);
        assert_eq!(run.code, 64, "{}", run.stderr);
        assert!(
            run.stderr.starts_with("Error: The input isn't valid JSON.  [VL0902]"),
            "{}",
            run.stderr
        );
        assert!(!run.stderr.contains("goal Add"), "{}", run.stderr);
    }
}

/// `velme check` leaves an entry stale from a source edit to `velme build`, even with the store gone; a current entry
/// without its artifact is an error.
#[test]
fn check_skips_entries_the_source_changed_even_without_a_store() {
    let project = copy("add", "no_store");
    fs::remove_dir_all(project.join(VELME_DIR)).expect("store deleted");
    let file = project.join("add.velme");
    let path = file.to_str().expect("UTF-8 path");
    let run = velme(&["check", path]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(run.stderr.contains("[VL0701]"), "{}", run.stderr);
    fs::write(&file, read(&file).replace("Add a and b.", "Add both numbers.")).expect("edited");
    let run = velme(&["check", path]);
    assert_eq!((run.stderr.as_str(), run.code), ("", 0));
    assert!(
        run.stdout
            .ends_with("✓ IR valid        (0 goals locked)\n✓ Checks valid\n"),
        "{}",
        run.stdout
    );
}

/// The lock records a source file's path from the project root as text (R-CLI-19), so a name on that path that isn't
/// UTF-8 is a file error, not a lossy lock entry. Only Linux file systems accept such a name.
#[cfg(target_os = "linux")]
#[test]
fn a_path_that_isnt_utf8_is_a_file_error() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = scratch("not_utf8");
    fs::write(root.join(CONFIG_FILE), "").expect("config");
    let dir = root.join(OsStr::from_bytes(b"bad\xff"));
    fs::create_dir_all(&dir).expect("directory");
    fs::write(dir.join("add.velme"), read(&repo(EXAMPLE))).expect("source");
    std::os::unix::fs::symlink(&dir, root.join("link")).expect("linked");
    let run = velme_in(&root, &["check", "link/add.velme"], b"");
    assert!(
        run.stderr.contains("VL0901") && run.stderr.contains("isn't UTF-8"),
        "{}",
        run.stderr
    );
    assert_ne!(run.code, 0);
}

/// `x / 0` in a leaf is `VL0602` with exit status 3, and nothing that looks like a result is printed, in either output
/// form (AC-RUN-08).
#[test]
fn ac_run_08_division_by_zero_prints_no_value() {
    let dir = scratch("division_by_zero");
    let source = "language: velme/0.1\n\ngoal Boom(x: Number) -> Number:\n    plan: \"Divide x by zero.\"\n";
    fs::write(dir.join("boom.velme"), source).expect("source");
    let program = program(source);
    let number = serde_json::json!({"t": "Number"});
    let zero = serde_json::json!({"kind": "literal", "type": number, "value": 0});
    let body =
        serde_json::json!({"kind": "binary", "op": "div", "left": {"kind": "input", "name": "x"}, "right": zero});
    let ir = serde_json::json!({"ir_version": "0.1", "builtins_version": "0.1", "goal": "Boom", "types": {},
        "inputs": [["x", number]], "output": number, "calls": [], "body": body});
    install(&dir, "boom.velme", &program, &ir.to_string());
    let file = dir.join("boom.velme");
    let file = file.to_str().expect("UTF-8 path");
    let human = velme(&["run", file, "--goal", "Boom", "--arg", "x=7"]);
    assert_eq!(human.code, 3, "{}", human.stderr);
    assert!(!human.stdout.contains("Result"), "{}", human.stdout);
    assert!(
        human.stderr.contains("`Boom` tried to divide 7 by 0"),
        "{}",
        human.stderr
    );
    let machine = velme(&["run", file, "--goal", "Boom", "--arg", "x=7", "--json"]);
    assert_eq!(machine.code, 3);
    let envelope = json(&machine);
    let result = &envelope["results"][0];
    assert_eq!(result["status"], "failed");
    assert_eq!(result["diagnostics"][0]["code"], "VL0602");
    assert!(result.get("result").is_none_or(Value::is_null), "{result}");
}

// ---- velme explain, velme trace ----

/// Every example of `examples/` with the goal and input the determinism tests run it on (AC-QA-05): its fixture project
/// (a copy of the example with hand-written IR, D-16), the goal and the `--arg`s.
const RUNS: [(&str, &str, &str, &[&str]); 7] = [
    ("add", "add.velme", "Add", &["a=2", "b=3"]),
    ("hello", "hello.velme", "SayHello", &[r#"name="Lina""#]),
    (
        "find_badge",
        "find_badge.velme",
        "FindBadge",
        &[r#"player={"name":"Lina","score":820}"#],
    ),
    ("double_then_add_one", "double_then_add_one.velme", "Main", &["x=4"]),
    (
        "player_summary",
        "player_summary.velme",
        "BuildPlayerSummary",
        &[r#"player={"name":"Lina","jump_height":3,"score":820}"#],
    ),
    (
        "level_summary",
        "level_summary.velme",
        "CreateLevelSummary",
        &[r#"level={"enemy_count":7,"treasure_count":3,"base_score":120}"#],
    ),
    (
        "order_total",
        "order_total.velme",
        "OrderTotal",
        &[r#"items=[{"price":10,"quantity":3},{"price":5,"quantity":1}]"#],
    ),
];

/// The JSON with every timing field, which differs from run to run, removed (`runtime/30` §8): they end in `_us`.
fn without_durations(json: &Value) -> Value {
    match json {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !key.ends_with("_us"))
                .map(|(key, value)| (key.clone(), without_durations(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_durations).collect()),
        other => other.clone(),
    }
}

/// `velme trace --json` of a fixture project's goal with `jobs` workers.
fn trace(name: &str, file: &str, goal: &str, args: &[&str], jobs: &str) -> Run {
    let file = example_file(name, file);
    let mut command = vec!["trace", file.as_str(), "--goal", goal, "--jobs", jobs, "--json"];
    for arg in args {
        command.extend(["--arg", arg]);
    }
    velme(&command)
}

/// The explanation of the three launch demos is the golden text: waves as "First / Then / At the same time / Finally",
/// each call by its goal in words and its plan's first sentence (AC-RUN-10, R-GOAL-16, R-RUN-22, R-RUN-23). A project
/// with no lock is enough: it reads no artifact.
#[test]
fn ac_run_10_explain_matches_the_golden_text() {
    let cases = [
        ("examples/intermediate/player_summary.velme", "BuildPlayerSummary"),
        ("examples/games/level_summary.velme", "CreateLevelSummary"),
        ("examples/professional/order_total.velme", "OrderTotal"),
    ];
    for (file, goal) in cases {
        let run = velme(&["explain", file, "--goal", goal]);
        assert_eq!((run.stderr.as_str(), run.code), ("", 0), "{goal}");
        insta::assert_snapshot!(format!("explain_{goal}"), run.stdout);
    }
    // All three parallel calls of a goal are one group (R-RUN-23), and a leaf has none.
    let run = velme(&["explain", "examples/beginner/find_badge.velme", "--goal", "FindBadge"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains("This goal calls no others: Give the player Gold"),
        "{}",
        run.stdout
    );
}

/// `velme explain` is byte-identical across runs, needs no lock and no artifact, and changes nothing. It takes a
/// program and a goal and nothing else (`velme_runtime::explain`), and the CLI builds no provider before `velme build`
/// (M5b), so it cannot call one (AC-CLI-07, AC-CMP-02).
#[test]
fn ac_cli_07_explain_is_byte_identical_and_writes_nothing() {
    let project = copy("player_summary", "ac_cli_07");
    let before = tree(&project);
    let file = project.join("player_summary.velme");
    let file = file.to_str().expect("UTF-8 path");
    let first = velme(&["explain", file, "--goal", "BuildPlayerSummary"]);
    assert_eq!(first.code, 0, "{}", first.stderr);
    for _ in 0..20 {
        let again = velme(&["explain", file, "--goal", "BuildPlayerSummary"]);
        assert_eq!(
            (&again.stdout, &again.stderr, again.code),
            (&first.stdout, &first.stderr, 0)
        );
    }
    // Without the store and the lock, the same words.
    let bare = velme(&[
        "explain",
        "examples/intermediate/player_summary.velme",
        "--goal",
        "BuildPlayerSummary",
    ]);
    assert_eq!(bare.stdout, first.stdout);
    assert_eq!(tree(&project), before);
    let json = velme(&["explain", file, "--goal", "BuildPlayerSummary", "--json"]);
    assert_eq!(json.code, 0, "{}", json.stderr);
    assert_eq!(self::json(&json)["results"][0]["result"], first.stdout);
    let wrong = velme(&["explain", file, "--goal", "Nope"]);
    assert_eq!(wrong.code, 64, "{}", wrong.stderr);
}

/// The trace, without durations, is byte-identical for `--jobs 1` and `--jobs 8` on the golden programs, and lists the
/// calls in source order (AC-RUN-04 trace half, AC-CLI-06). The two failing siblings are in
/// `failed_siblings_report_the_lowest_source_order_failure`.
#[test]
fn ac_run_04_the_trace_is_the_same_for_every_jobs_value() {
    for (name, file, goal, args) in RUNS {
        let one = trace(name, file, goal, args, "1");
        let eight = trace(name, file, goal, args, "8");
        assert_eq!((one.code, eight.code), (0, 0), "{name}: {}", one.stderr);
        assert_eq!(
            without_durations(&json(&one)).to_string(),
            without_durations(&json(&eight)).to_string(),
            "{name}"
        );
    }
    // Source order, not completion order: the five calls of the games example, whatever finished first.
    let level = &RUNS[5];
    let run = trace(level.0, level.1, level.2, level.3, "8");
    let envelope = json(&run);
    let calls = envelope["results"][0]["trace"]["goal"]["calls"]
        .as_array()
        .expect("calls");
    let bindings: Vec<&str> = calls.iter().map(|c| c["binding"].as_str().expect("binding")).collect();
    assert_eq!(bindings, ["enemies", "treasures", "score", "difficulty", "reward"]);
    let waves: Vec<u64> = calls.iter().map(|c| c["wave"].as_u64().expect("wave")).collect();
    assert_eq!(waves, [1, 1, 1, 2, 2]);
    // The output as printed, so the snapshot shows the order of the fields; only the timing lines are left out.
    let timing = ["\"start_us\"", "\"end_us\"", "\"duration_us\""];
    let printed: String = run
        .stdout
        .lines()
        .filter(|line| !timing.iter().any(|key| line.trim_start().starts_with(key)))
        .map(|line| format!("{line}\n"))
        .collect();
    insta::assert_snapshot!("trace_level_summary_json", printed);
}

/// The human trace of the parallel demo, calls in source order with each goal's nested event (`runtime/30` §8).
#[test]
fn trace_prints_every_call_in_source_order() {
    let file = example_file("player_summary", "player_summary.velme");
    let player = r#"player={"name":"Lina","jump_height":3,"score":820}"#;
    let first = velme(&[
        "trace",
        &file,
        "--goal",
        "BuildPlayerSummary",
        "--arg",
        player,
        "--jobs",
        "1",
    ]);
    let second = velme(&[
        "trace",
        &file,
        "--goal",
        "BuildPlayerSummary",
        "--arg",
        player,
        "--jobs",
        "8",
    ]);
    assert_eq!((first.code, first.stderr.as_str()), (0, ""));
    assert_eq!(first.stdout, second.stdout);
    let score = first.stdout.find("score = CalculateScore").expect("the first call");
    let badge = first.stdout.find("badge = FindBadge").expect("the second call");
    assert!(score < badge, "{}", first.stdout);
    insta::assert_snapshot!(first.stdout);
}

/// Each example of `examples/`, 50 times over with the workers varying, gives the same result and the same trace
/// without durations (AC-QA-05, AC-RDM-09 interpreter half, INV-3).
#[test]
fn ac_qa_05_every_example_is_deterministic_across_fifty_runs() {
    let mut examples: Vec<String> = Vec::new();
    for group in fs::read_dir(repo("examples")).expect("examples") {
        let group = group.expect("entry").path();
        if group.is_dir() {
            for file in fs::read_dir(&group).expect("group") {
                let file = file.expect("entry").path();
                if file.extension().is_some_and(|e| e == "velme") {
                    examples.push(file.file_name().expect("name").to_string_lossy().into_owned());
                }
            }
        }
    }
    examples.sort();
    let mut covered: Vec<String> = RUNS.iter().map(|r| r.1.to_owned()).collect();
    covered.sort();
    assert_eq!(examples, covered, "every example has a run in RUNS");
    let jobs = ["1", "2", "3", "8"];
    for (name, file, goal, args) in RUNS {
        let mut reference: Option<String> = None;
        for i in 0..50 {
            let run = trace(name, file, goal, args, jobs[i % jobs.len()]);
            assert_eq!(run.code, 0, "{name} run {i}: {}", run.stderr);
            let seen = without_durations(&json(&run)).to_string();
            assert_eq!(reference.get_or_insert_with(|| seen.clone()), &seen, "{name} run {i}");
        }
    }
}

/// Records come out with their fields in declaration order, as the sample of `tooling/40` §3.3 shows: in the result of
/// `velme run`, in its `--json` envelope and in every value of a trace (R-TYP-23).
#[test]
fn records_keep_their_declaration_order() {
    let player = r#"player={"name":"Lina","jump_height":3,"score":820}"#;
    let file = example_file("player_summary", "player_summary.velme");
    let run = velme(&["run", &file, "--goal", "BuildPlayerSummary", "--arg", player]);
    assert!(
        run.stdout
            .ends_with("Result:\n{\n  \"name\": \"Lina\",\n  \"score\": 820,\n  \"badge\": \"Silver\"\n}\n"),
        "{}",
        run.stdout
    );
    let json = velme(&[
        "trace",
        &file,
        "--goal",
        "BuildPlayerSummary",
        "--arg",
        player,
        "--json",
    ]);
    let compact: String = json.stdout.split_whitespace().collect();
    assert!(
        compact.contains(r#"{"name":"Lina","jump_height":3,"score":820}"#),
        "{}",
        json.stdout
    );
    assert!(
        compact.contains(r#"{"name":"Lina","score":820,"badge":"Silver"}"#),
        "{}",
        json.stdout
    );
}

/// A value past `max_output_bytes` is left out of the trace with `truncated: true`, never written as `null`, which is
/// `nothing` (R-RUN-20).
#[test]
fn a_value_past_the_output_limit_is_truncated_in_the_trace() {
    let file = example_file("hello", "hello.velme");
    let name = "a".repeat(1_100_000);
    let input = format!(r#"{{"name": "{name}"}}"#);
    let run = velme_in(
        &repo(""),
        &["trace", &file, "--goal", "SayHello", "--input", "-", "--json"],
        input.as_bytes(),
    );
    assert_eq!(run.code, 3, "{}", &run.stderr[..run.stderr.len().min(500)]);
    let trace = &json(&run)["results"][0]["trace"]["goal"];
    assert_eq!(trace["inputs"][0]["name"], "name");
    assert_eq!(trace["inputs"][0]["truncated"], true);
    assert!(trace["inputs"][0].get("value").is_none());
    assert_eq!(trace["outcome"], "failed");
    assert_eq!(trace["failure"]["code"], "VL0606");
}
