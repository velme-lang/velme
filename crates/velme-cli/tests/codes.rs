//! One human and one JSON snapshot for each diagnostic code `reference/90` lists, triggered through the CLI (R-QA-05,
//! D-140). Codes no other snapshot shows are here; `VL0603` and `VL0801` are at library level, in `velme-runtime` and
//! `velme-wasm`, and `velme-diagnostics`'s AC-QA-04 test diffs the codes against every snapshot. Needs the
//! `test-provider` feature for the scripted builds, which the gate turns on (D-94).
#![cfg(feature = "test-provider")]
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use velme_builtins::BUILTINS_VERSION;
use velme_ir::IR_VERSION;
use velme_test_support::backend::{Config, Mode, On, Server};
use velme_test_support::ir_json::{binary, builtin, input, literal, number, numbers};
use velme_test_support::{install, program, repo, velme_command};

/// A leaf goal for the build codes.
const LEAF: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    examples:
        - Double(2) == 4
";

/// [`LEAF`] and a wired goal calling it, whose own example is wrong.
const WIRED: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    examples:
        - Double(2) == 4

goal Both(n: Number) -> Number:
    call:
        result = Double(n)
    examples:
        - Both(1) == 3
";

/// A goal for each runtime code: out of fuel, divided by zero, out of memory, a list too long, and a body the WASM
/// backend declines.
const RUNTIME: &str = "language: velme/0.1

goal Spin(n: Number) -> Number:
    budget cpu=1ms
    plan: \"Count up to n times n.\"

goal Divide(n: Number) -> Number:
    plan: \"Divide n by zero.\"

goal Hungry(n: Number) -> Number:
    budget memory=1kb
    plan: \"Add up the numbers below n.\"

goal Long(n: Number) -> Number:
    plan: \"Add up the numbers below n.\"

goal Absent(xs: List<Number>) -> Boolean:
    plan: \"Whether nothing is found.\"
";

/// The file the projects keep their source in.
const FILE: &str = "game.velme";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

/// An empty directory for `test`, under this process's id so concurrent runs don't share it.
fn scratch(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("codes-{}", std::process::id()))
        .join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("project directory");
    dir
}

/// A project for `test` whose one file holds `source`.
fn project(test: &str, source: &str) -> PathBuf {
    let dir = scratch(test);
    fs::write(dir.join(FILE), source).expect("source");
    dir
}

/// A copy of the committed fixture project `add` for `test`.
fn add(test: &str) -> PathBuf {
    let dir = scratch(test);
    copy_tree(&repo("tests/fixtures/run/add"), &dir);
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("directory").filter_map(Result::ok) {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            fs::create_dir_all(&target).expect("directory");
            copy_tree(&path, &target);
        } else {
            fs::copy(&path, &target).expect("copied");
        }
    }
}

/// `velme args` in `dir` with `envs` set. The user's own provider settings and config file are out of reach, so no
/// test can reach a real provider and none depends on the machine.
fn velme(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Out {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("codes-home");
    fs::create_dir_all(&home).expect("an empty config home");
    let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_SCRIPT")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_URL")
        .env_remove("VELME_EXTERNAL_TOKEN")
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .envs(envs.iter().copied())
        .output()
        .expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

/// Runs `args` in `dir` once as is and once with `--json`, checks both show `code` and exit alike, and snapshots the
/// human rendering (stderr) and the JSON envelope as `<code>_human` and `<code>_json`, each `(text, placeholder)` of
/// `redact` replaced: what depends on the machine.
fn snapshot_pair(code: &str, dir: &Path, args: &[&str], envs: &[(&str, &str)], redact: &[(&str, &str)]) {
    let redact = |text: &str| redact.iter().fold(text.to_owned(), |t, (from, to)| t.replace(from, to));
    let human = velme(dir, args, envs);
    assert!(
        human.stderr.contains(&format!("  [{code}]")),
        "{}{}",
        human.stdout,
        human.stderr
    );
    let json = velme(dir, &[args, &["--json"]].concat(), envs);
    assert_eq!(json.code, human.code, "{}", json.stdout);
    assert_eq!(json.stderr, "", "--json writes nothing to stderr");
    let envelope: Value = serde_json::from_str(&json.stdout).expect("--json prints one JSON document");
    velme_test_support::schema::assert_cli_envelope(&envelope);
    // Rendered by serde_json, not insta's serializer: with `arbitrary_precision` insta would print each number as
    // serde_json's private token map.
    let pretty = serde_json::to_string_pretty(&envelope).expect("JSON value serializes");
    assert!(pretty.contains(&format!("\"code\": \"{code}\"")), "{pretty}");
    let name = code.to_lowercase();
    insta::assert_snapshot!(format!("{name}_human"), redact(&human.stderr));
    insta::assert_snapshot!(format!("{name}_json"), redact(&pretty));
}

/// A reply holding the body `n * 2` (D-103).
fn double() -> String {
    json!({"body": {"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                    "right": {"kind": "literal", "type": {"t": "Number"}, "value": 2}}})
    .to_string()
}

/// Writes a script of replies (`tooling/40` §5.2) into `dir` and returns its path as the variable's value.
fn script(dir: &Path, replies: &[String]) -> String {
    let path = dir.join("script.json");
    fs::write(&path, serde_json::to_string(replies).expect("json")).expect("script");
    path.to_str().expect("a UTF-8 path").to_owned()
}

/// `build` on the scripted provider with `replies`, as both renderings.
fn scripted(code: &str, test: &str, source: &str, replies: &[String]) {
    let dir = project(test, source);
    let script = script(&dir, replies);
    snapshot_pair(
        code,
        &dir,
        &["build", FILE, "--provider", "scripted"],
        &[("VELME_SYNTH_SCRIPT", &script)],
        &[],
    );
}

/// `build` on the `external` provider, its backend answering `Double` with `reply` and misbehaving as `mode` says.
fn external(code: &str, test: &str, reply: &str, mode: Option<Mode>) {
    let dir = project(test, LEAF);
    let replies = dir.join("replies");
    fs::create_dir_all(&replies).expect("replies");
    fs::write(replies.join("Double.json"), reply).expect("reply");
    let config = Config::replying(&replies);
    let server = Server::start(match mode {
        Some(mode) => config.misbehaving(mode, On::Synthesize),
        None => config,
    });
    // The output names only the host; the port is redacted in case that changes.
    let url = server.url().to_owned();
    snapshot_pair(
        code,
        &dir,
        &["build", FILE, "--provider", "external", "--external-url", &url],
        &[],
        &[(&url, "[url]")],
    );
}

// ---- syntax, names and types, calls ----

/// Each file under `tests/golden/diagnostics` is named for the one code `velme check` reports on it.
#[test]
fn check_codes() {
    insta::glob!("../../../tests/golden/diagnostics", "*.velme", |path| {
        let name = path.file_name().and_then(|n| n.to_str()).expect("UTF-8 file name");
        let code = name.trim_end_matches(".velme").to_uppercase();
        let rel = format!("tests/golden/diagnostics/{name}");
        let human = velme(&repo(""), &["check", &rel], &[]);
        assert_ne!(human.code, 0, "{}", human.stderr);
        assert!(human.stderr.contains(&format!("  [{code}]")), "{}", human.stderr);
        let json = velme(&repo(""), &["check", "--json", &rel], &[]);
        assert_eq!(json.code, human.code, "{}", json.stdout);
        assert_eq!(json.stderr, "", "--json writes nothing to stderr");
        let envelope: Value = serde_json::from_str(&json.stdout).expect("--json prints one JSON document");
        velme_test_support::schema::assert_cli_envelope(&envelope);
        let pretty = serde_json::to_string_pretty(&envelope).expect("JSON value serializes");
        let codes: Vec<&str> = envelope["diagnostics"]
            .as_array()
            .expect("a diagnostics array")
            .iter()
            .filter_map(|d| d["code"].as_str())
            .collect();
        assert_eq!(codes, [code.as_str()], "{pretty}");
        insta::assert_snapshot!("human", human.stderr);
        insta::assert_snapshot!("json", pretty);
    });
}

// ---- synthesis ----

#[test]
fn vl0403_no_reply_passes_within_the_tries() {
    let wrong = || json!({"body": {"kind": "input", "name": "n"}}).to_string();
    let replies = [r#"{"nope": 1}"#.to_owned(), wrong(), wrong(), wrong()];
    scripted("VL0403", "vl0403", LEAF, &replies);
}

#[test]
fn vl0404_offline_with_a_goal_to_build() {
    let dir = project("vl0404", LEAF);
    snapshot_pair("VL0404", &dir, &["build", FILE, "--offline"], &[], &[]);
}

#[test]
fn vl0405_no_provider_is_set_up() {
    let dir = project("vl0405", LEAF);
    snapshot_pair("VL0405", &dir, &["build", FILE], &[], &[]);
}

#[test]
fn vl0406_the_backend_replies_with_garbage() {
    external("VL0406", "vl0406", &double(), Some(Mode::Garbage));
}

#[test]
fn vl0407_a_question_instead_of_ir() {
    scripted("VL0407", "vl0407", LEAF, &[r#"{"question": "Round up?"}"#.to_owned()]);
}

#[test]
fn vl0408_the_backend_has_no_answer_yet() {
    external("VL0408", "vl0408", r#"{"pending": "ticket 42"}"#, None);
}

#[test]
fn vl0409_a_caller_of_an_unbuilt_goal_is_blocked() {
    scripted("VL0409", "vl0409", WIRED, &[r#"{"question": "Round up?"}"#.to_owned()]);
}

// ---- checks ----

#[test]
fn vl0501_a_check_fails_on_run() {
    let dir = add("vl0501");
    broken_add(&dir);
    snapshot_pair(
        "VL0501",
        &dir,
        &["run", "add.velme", "--goal", "Add", "--arg", "a=2", "--arg", "b=3"],
        &[],
        &[],
    );
}

#[test]
fn vl0502_an_example_fails_in_test() {
    let dir = add("vl0502");
    // `Add(2, 3) == 5` is the one example the artifact gets wrong, and its run fails the check too: both are reported.
    broken_add(&dir);
    snapshot_pair("VL0502", &dir, &["test", "add.velme", "--locked"], &[], &[]);
}

#[test]
fn vl0503_a_wired_goal_fails_its_own_example() {
    scripted("VL0503", "vl0503", WIRED, &[double()]);
}

/// Replaces the artifact of `Add` in the copy `dir` with one that is right except at `a == 2`, where it adds one more.
fn broken_add(dir: &Path) {
    fs::remove_dir_all(dir.join(".velme")).expect("store removed");
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let source = fs::read_to_string(dir.join("add.velme")).expect("source");
    let sum = binary("add", input("a"), input("b"));
    let body = json!({"kind": "if", "cond": binary("eq", input("a"), literal(2)),
                      "then": binary("add", sum.clone(), literal(1)), "else": sum});
    let inputs = json!([["a", number()], ["b", number()]]);
    install(
        dir,
        "add.velme",
        &program(&source),
        &ir("Add", &inputs, &number(), &body),
    );
    fs::remove_dir_all(dir.join(".velme/tmp")).expect("temporary directory");
}

/// The IR document of goal `goal`, which makes no calls.
fn ir(goal: &str, inputs: &Value, output: &Value, body: &Value) -> String {
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": goal, "types": {},
           "inputs": inputs, "output": output, "body": body})
    .to_string()
}

// ---- runtime ----

/// A project with [`RUNTIME`] and hand-written IR installed for each of its goals (D-16).
fn runtime(test: &str) -> PathBuf {
    let dir = project(test, RUNTIME);
    let program = program(RUNTIME);
    let n = || json!([["n", number()]]);
    let local = |name: &str| json!({"kind": "local", "name": name});
    // `n * n`, counted up by one per element of `range(n)` inside each element of `range(n)`.
    let range = builtin("range", &[input("n")]);
    let inner = json!({"kind": "reduce", "list": range, "init": local("outer"),
                       "fn": {"acc": "sum", "param": "j", "body": binary("add", local("sum"), literal(1))}});
    let count = json!({"kind": "reduce", "list": range, "init": literal(0),
                       "fn": {"acc": "outer", "param": "i", "body": inner}});
    let sum = builtin("sum", std::slice::from_ref(&range));
    // `is_empty(find(map(xs, x => nothing), y => true))`: a list of `Nothing`, which the WASM backend has no code for.
    let nothing = json!({"kind": "literal", "type": {"t": "Nothing"}, "value": null});
    let mapped = json!({"kind": "map", "list": input("xs"), "fn": {"param": "x", "body": nothing}});
    let found = json!({"kind": "find", "list": mapped,
                       "fn": {"param": "y", "body": {"kind": "literal", "type": {"t": "Boolean"}, "value": true}}});
    let absent = json!({"kind": "unary", "op": "is_empty", "arg": found});
    let goals = [
        ("Spin", n(), number(), count),
        ("Divide", n(), number(), binary("div", input("n"), literal(0))),
        ("Hungry", n(), number(), sum.clone()),
        ("Long", n(), number(), sum),
        ("Absent", json!([["xs", numbers()]]), json!({"t": "Boolean"}), absent),
    ];
    for (goal, inputs, output, body) in goals {
        install(&dir, FILE, &program, &ir(goal, &inputs, &output, &body));
    }
    fs::remove_dir_all(dir.join(".velme/tmp")).expect("temporary directory");
    dir
}

/// `run` of `goal` on `args` in a fresh [`runtime`] project, as both renderings.
fn run(code: &str, goal: &str, args: &[&str]) {
    let dir = runtime(&code.to_lowercase());
    snapshot_pair(code, &dir, &[&["run", FILE, "--goal", goal], args].concat(), &[], &[]);
}

#[test]
fn vl0601_a_goal_out_of_fuel() {
    // 300 × 300 elements at four fuel each is more than `cpu=1ms`, 100 000 fuel.
    run("VL0601", "Spin", &["--arg", "n=300"]);
}

#[test]
fn vl0602_division_by_zero() {
    run("VL0602", "Divide", &["--arg", "n=5"]);
}

#[test]
fn vl0604_a_goal_out_of_memory() {
    run("VL0604", "Hungry", &["--arg", "n=1000"]);
}

#[test]
fn vl0606_a_list_past_the_size_limit() {
    run("VL0606", "Long", &["--arg", "n=20000"]);
}

#[test]
fn vl0607_a_leaf_the_wasm_backend_declines() {
    run("VL0607", "Absent", &["--arg", "xs=[1, 2]", "--backend", "wasm"]);
}

// ---- artifacts ----

/// `run` of `Add` in the copy `dir` of the fixture project, as both renderings.
fn run_add(code: &str, dir: &Path) {
    let args = ["run", "add.velme", "--goal", "Add", "--arg", "a=1", "--arg", "b=2"];
    snapshot_pair(code, dir, &args, &[], &[]);
}

/// The one artifact of the copy `dir`.
fn artifact(dir: &Path) -> PathBuf {
    let store = dir.join(".velme/artifacts");
    let mut files = fs::read_dir(&store).expect("the store").filter_map(Result::ok);
    let only = files.next().expect("an artifact").path();
    assert!(files.next().is_none(), "one artifact");
    only
}

#[test]
fn vl0701_the_locked_artifact_is_missing() {
    let dir = add("vl0701");
    fs::remove_file(artifact(&dir)).expect("artifact removed");
    run_add("VL0701", &dir);
}

#[test]
fn vl0702_the_goal_changed_since_its_build() {
    let dir = add("vl0702");
    let source = fs::read_to_string(dir.join("add.velme")).expect("source");
    fs::write(
        dir.join("add.velme"),
        source.replace("Add a and b.", "Add both numbers."),
    )
    .expect("edited");
    run_add("VL0702", &dir);
}

#[test]
fn vl0703_the_artifact_was_changed() {
    let dir = add("vl0703");
    let path = artifact(&dir);
    let mut bytes = fs::read(&path).expect("artifact");
    let last = bytes.len() - 2;
    bytes[last] ^= 0x01;
    fs::write(&path, bytes).expect("edited");
    run_add("VL0703", &dir);
}

// ---- files and input ----

#[test]
fn vl0901_the_source_file_is_missing() {
    let dir = project("vl0901", LEAF);
    // The note is the operating system's own message, which differs between systems.
    let os = fs::read(dir.join("missing.velme"))
        .expect_err("no such file")
        .to_string();
    let redact = [(os.as_str(), "[the system's message]")];
    snapshot_pair("VL0901", &dir, &["check", "missing.velme"], &[], &redact);
}

#[test]
fn vl0902_an_input_of_the_wrong_type() {
    let dir = add("vl0902");
    let args = [
        "run",
        "add.velme",
        "--goal",
        "Add",
        "--arg",
        "a=\"two\"",
        "--arg",
        "b=3",
    ];
    snapshot_pair("VL0902", &dir, &args, &[], &[]);
}

#[test]
fn vl0903_no_goal_by_that_name() {
    let dir = add("vl0903");
    let args = ["run", "add.velme", "--goal", "Ad", "--arg", "a=1", "--arg", "b=2"];
    snapshot_pair("VL0903", &dir, &args, &[], &[]);
}
