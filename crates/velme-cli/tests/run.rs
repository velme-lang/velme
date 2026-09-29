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
use velme_test_support::{goal_id, install, program, read, repo};

/// Set to rewrite the committed fixture projects from their hand-written IR, then review the diff.
const BLESS: &str = "VELME_BLESS_FIXTURES";

/// The single-goal fixture projects: each is `examples/beginner/add.velme` with the IR of
/// `tests/fixtures/run/<name>.json`.
const FIXTURES: [&str; 2] = ["add", "add_broken"];

/// The fixture projects made from an example of the launch demos, with the IR of each of its goals in
/// `tests/fixtures/run/<name>.ir/<Goal>.json`: the example's path by fixture name.
const EXAMPLES: [(&str, &str); 5] = [
    ("find_badge", "examples/beginner/find_badge.velme"),
    ("double_then_add_one", "examples/beginner/double_then_add_one.velme"),
    ("player_summary", "examples/intermediate/player_summary.velme"),
    ("level_summary", "examples/games/level_summary.velme"),
    ("order_total", "examples/professional/order_total.velme"),
];

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
        .chain(EXAMPLES.iter().map(|(name, example)| {
            let goals = fs::read_dir(repo(&format!("tests/fixtures/run/{name}.ir")))
                .expect("IR")
                .count();
            (*name, made_example(name, example), goals)
        }));
    for (name, project, goals) in projects {
        let committed = repo(&format!("tests/fixtures/run/{name}"));
        let made = tree(&project);
        if std::env::var_os(BLESS).is_some() {
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

/// `velme check` reads the store only to validate locked IR and writes nothing (`tooling/40` §2). No provider exists
/// before M5a, so the panicking-provider half of the criterion arrives with the `SynthProvider` trait.
#[test]
fn ac_cmp_02_check_reads_the_store_only_to_validate_locked_ir() {
    let project = copy("add", "ac_cmp_02");
    let before = tree(&project);
    let file = project.join("add.velme");
    let run = velme(&["check", file.to_str().expect("UTF-8 path")]);
    assert_eq!(
        (run.stdout.as_str(), run.stderr.as_str(), run.code),
        (
            "✓ Parsed\n✓ Types valid\n✓ Call graph valid\n✓ IR valid        (1 goal locked)\n",
            "",
            0
        )
    );
    assert_eq!(tree(&project), before);
    // A file with no lock in its project reads no store: the output is phases 1–6 alone.
    let run = velme(&["check", EXAMPLE]);
    assert_eq!(run.stdout, "✓ Parsed\n✓ Types valid\n✓ Call graph valid\n");
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
                .contains(&format!("`{goal}` changed since it was last built")),
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
        ("Main  ✓\n\nResult:\n9\n", "", 0)
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
    assert_eq!(run.stdout, "OrderTotal  ✓\n\nResult:\n270\n");
}

/// The result is byte-identical for `--jobs 1` and `--jobs 8` on the golden programs (AC-RUN-04, result half; the trace
/// half follows the trace model in M4c).
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
third = Later(first)\n    plan: \"Add them.\"\n";
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
    let human = velme(&["run", file, "--goal", "Both", "--arg", "n=3"]);
    assert_eq!(human.code, 3);
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
        ("Add  ✓ 3 examples\n", "", 0)
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
        run.stdout.ends_with("✓ IR valid        (0 goals locked)\n"),
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
