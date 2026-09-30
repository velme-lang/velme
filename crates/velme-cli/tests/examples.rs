//! Every example builds, and rebuilds for free, on the `replay` provider (`compiler/22` R-SYNTH-43, D-94, D-99;
//! AC-RDM-01, AC-RDM-08): the replay fixtures under `tests/fixtures/synth` are recorded, unattended, from the test
//! `external` backend, an HTTP service, answering with the hand-written IR of `tests/fixtures/run`. A fixture that is out of date fails
//! here; `VELME_BLESS_FIXTURES=1` rewrites them, and the diff is then reviewed.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use velme_test_support::backend::{Config, Server};
use velme_test_support::repo;

/// Set to rewrite the committed fixtures, then review the diff.
const BLESS: &str = "VELME_BLESS_FIXTURES";

/// The examples, each with where the hand-written IR of its goals is: a directory of `<Goal>.json`, or the one file
/// `tests/fixtures/run/add.json` for `add`.
const EXAMPLES: [(&str, &str); 7] = [
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

const FIXTURES: &str = "tests/fixtures/synth";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn velme(dir: &Path, args: &[&str]) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_velme"))
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_URL")
        .env_remove("VELME_EXTERNAL_TOKEN")
        .output()
        .expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

fn stem(example: &str) -> &str {
    Path::new(example)
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("a file name")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-examples").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// Every file under `dir`, by path from `dir` with `/` separators, without `.gitkeep`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, into);
            } else if path.file_name().is_some_and(|n| n != ".gitkeep") {
                let name = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                into.insert(name, fs::read(&path).expect("readable"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// A project of the example at `example`, alone, in a fresh directory called `name`.
fn project(example: &str, name: &str) -> (PathBuf, String) {
    let dir = scratch(name);
    let file = format!("{}.velme", stem(example));
    fs::copy(repo(example), dir.join(&file)).expect("the example is copied");
    (dir, file)
}

/// The goals' replies as the test backend reads them: `<Goal>.json` in one directory.
fn replies(ir: &str, name: &str) -> PathBuf {
    let dir = scratch(&format!("{name}-replies"));
    let source = repo(ir);
    if source.is_dir() {
        for entry in fs::read_dir(&source).expect("IR directory").filter_map(Result::ok) {
            fs::copy(entry.path(), dir.join(entry.file_name())).expect("IR copied");
        }
    } else {
        fs::copy(&source, dir.join("Add.json")).expect("IR copied");
    }
    dir
}

/// What a build wrote for the lock and the store, for comparing two builds.
fn built(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = tree(&dir.join(".velme"));
    files.insert(
        "velme.lock".to_owned(),
        fs::read(dir.join("velme.lock")).expect("a lock"),
    );
    files
}

/// What recording every example wrote: the fixtures, and the lock and store of each example's build.
type Recorded = (BTreeMap<String, Vec<u8>>, BTreeMap<String, BTreeMap<String, Vec<u8>>>);

/// [`record_all`], once per test process: the tests that need it share the run.
fn record() -> &'static Recorded {
    static RECORDED: std::sync::OnceLock<Recorded> = std::sync::OnceLock::new();
    RECORDED.get_or_init(record_all)
}

/// Builds every example through the test backend with `VELME_SYNTH_RECORD=1`: the fixtures each recorded, and the lock
/// and store each wrote.
fn record_all() -> Recorded {
    let mut fixtures: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut stores = BTreeMap::new();
    for (example, ir) in EXAMPLES {
        let name = format!("{}-record", stem(example));
        let (dir, file) = project(example, &name);
        let server = Server::start(Config::replying(replies(ir, &name)));
        let out = Command::new(env!("CARGO_BIN_EXE_velme"))
            .args(["build", &file, "--provider", "external", "--external-url", server.url()])
            .current_dir(&dir)
            .env("VELME_SYNTH_RECORD", "1")
            .env_remove("VELME_EXTERNAL_URL")
            .env_remove("VELME_EXTERNAL_TOKEN")
            .output()
            .expect("velme runs");
        assert!(
            out.status.success(),
            "{example}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for (path, bytes) in tree(&dir.join(FIXTURES)) {
            // Every example records the same identity; the goal files are told apart by their keys.
            if let Some(earlier) = fixtures.insert(path.clone(), bytes.clone()) {
                assert_eq!(earlier, bytes, "{example}: {path} differs between examples");
            }
        }
        stores.insert(example.to_owned(), built(&dir));
    }
    (fixtures, stores)
}

/// The examples the fixtures are recorded for are the examples there are.
#[test]
fn every_example_has_its_ir_and_no_example_is_missing() {
    fn find(dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("directory").filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                find(&path, found);
            } else if path.extension().is_some_and(|e| e == "velme") {
                let relative = path.strip_prefix(repo("")).expect("in the repository");
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut found = Vec::new();
    find(&repo("examples"), &mut found);
    found.sort();
    let mut listed: Vec<String> = EXAMPLES.iter().map(|(example, _)| (*example).to_owned()).collect();
    listed.sort();
    assert_eq!(found, listed, "add the new example, with its IR, to EXAMPLES");
    for (_, ir) in EXAMPLES {
        assert!(repo(ir).exists(), "{ir}");
    }
}

/// The committed replay fixtures are what recording every example through the test backend gives now (D-99).
#[test]
fn the_committed_replay_fixtures_are_what_the_test_backend_records() {
    let (recorded, _) = record();
    let committed_dir = repo(FIXTURES);
    if std::env::var_os(BLESS).is_some() {
        for path in tree(&committed_dir).keys() {
            fs::remove_file(committed_dir.join(path)).expect("stale fixture removed");
        }
        for (path, bytes) in recorded {
            fs::write(committed_dir.join(path), bytes).expect("fixture written");
        }
    }
    let committed = tree(&committed_dir);
    assert!(
        committed == *recorded,
        "{FIXTURES} is out of date: run `{BLESS}=1 cargo test -p velme-cli --test examples` and review the diff"
    );
}

/// Each example builds on the replay provider, from a plan and nothing else, with its checks and examples passing; the
/// lock and the store are the ones the recorded build wrote (AC-RDM-01, AC-SYNTH-10).
#[test]
fn ac_rdm_01_every_example_builds_from_its_plan_on_replay() {
    let (_, stores) = record();
    for (example, _) in EXAMPLES {
        let (dir, file) = project(example, &format!("{}-replay", stem(example)));
        let fixtures = dir.join(FIXTURES);
        fs::create_dir_all(&fixtures).expect("fixture directory");
        for (path, bytes) in tree(&repo(FIXTURES)) {
            fs::write(fixtures.join(path), bytes).expect("fixture copied");
        }
        let out = velme(&dir, &["build", &file, "--provider", "replay"]);
        assert_eq!(out.code, 0, "{example}: {}{}", out.stdout, out.stderr);
        assert!(!out.stdout.contains('✗'), "{example}: {}", out.stdout);
        assert_eq!(built(&dir), stores[example], "{example}: replay built something else");
        // The built goals run their examples and checks.
        let tested = velme(&dir, &["test", &file]);
        assert_eq!(tested.code, 0, "{example}: {}{}", tested.stdout, tested.stderr);
    }
}

/// A second build of an unchanged example performs zero synthesis and contact, and leaves the lock and the store as they
/// were (AC-RDM-08, AC-ART-01, R-ART-17).
#[test]
fn ac_rdm_08_a_second_build_of_every_example_makes_no_provider_call() {
    for (example, _) in EXAMPLES {
        let (dir, file) = project(example, &format!("{}-twice", stem(example)));
        let fixtures = dir.join(FIXTURES);
        fs::create_dir_all(&fixtures).expect("fixture directory");
        for (path, bytes) in tree(&repo(FIXTURES)) {
            fs::write(fixtures.join(path), bytes).expect("fixture copied");
        }
        let first = velme(&dir, &["build", &file, "--provider", "replay"]);
        assert_eq!(first.code, 0, "{example}: {}{}", first.stdout, first.stderr);
        assert!(
            first.stderr.contains("Nothing is sent"),
            "{example}: the first build makes contact"
        );
        let before = built(&dir);
        // The fixtures are gone: a second build that asked for one would fail.
        fs::remove_dir_all(dir.join("tests")).expect("fixtures removed");
        let second = velme(&dir, &["build", &file, "--provider", "replay"]);
        assert_eq!(second.code, 0, "{example}: {}{}", second.stdout, second.stderr);
        assert!(
            second.stdout.contains("0 provider calls"),
            "{example}: {}",
            second.stdout
        );
        assert!(
            !second.stderr.contains("Nothing is sent"),
            "{example}: {}",
            second.stderr
        );
        assert_eq!(
            built(&dir),
            before,
            "{example}: the second build changed the lock or the store"
        );
    }
}
