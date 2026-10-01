//! Every example builds, and rebuilds for free, on the `replay` provider (`compiler/22` R-SYNTH-43, D-94, D-99;
//! AC-RDM-01, AC-RDM-08): the replay fixtures under `tests/fixtures/synth` are recorded, unattended, from the test
//! `external` backend, an HTTP service, answering with the bodies of the hand-written IR of `tests/fixtures/run`. A fixture that is out of date fails
//! here; `VELME_BLESS_FIXTURES=1` rewrites them, and the diff is then reviewed.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use velme_test_support::velme_command;

use velme_test_support::backend::{Config, Server};
use velme_test_support::differential::EXAMPLES;
use velme_test_support::repo;

/// Set to rewrite the committed fixtures, then review the diff.
const BLESS: &str = "VELME_BLESS_FIXTURES";

const FIXTURES: &str = "tests/fixtures/synth";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn velme(dir: &Path, args: &[&str]) -> Out {
    let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
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

/// The goals' replies as the test backend reads them: `<Goal>.json` in one directory, each the `body` of the hand-written
/// IR, which the backend sends as `{"body": …}` (D-103).
fn replies(ir: &str, name: &str) -> PathBuf {
    let dir = scratch(&format!("{name}-replies"));
    let source = repo(ir);
    let write = |file: &std::ffi::OsStr, from: &Path| {
        let goal: serde_json::Value = serde_json::from_str(&fs::read_to_string(from).expect("IR")).expect("IR JSON");
        fs::write(dir.join(file), goal["body"].to_string()).expect("body written");
    };
    if source.is_dir() {
        for entry in fs::read_dir(&source).expect("IR directory").filter_map(Result::ok) {
            write(&entry.file_name(), &entry.path());
        }
    } else {
        write("Add.json".as_ref(), &source);
    }
    dir
}

/// What a build wrote for the lock and the store, for comparing two builds. The synth log holds the time and latency of
/// each request, so it differs from build to build (R-SYNTH-23).
fn built(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = tree(&dir.join(".velme"));
    files.remove("synth-log.jsonl");
    files.insert(
        "velme.lock".to_owned(),
        fs::read(dir.join("velme.lock")).expect("a lock"),
    );
    files
}

/// What recording every example wrote: the fixtures, and the lock and store of each example's build.
type Recorded = (BTreeMap<String, Vec<u8>>, BTreeMap<String, BTreeMap<String, Vec<u8>>>);

/// [`record_all`], once per test process: the tests that need it share the run. Under `VELME_BLESS_FIXTURES=1` the
/// committed fixtures are rewritten with it here, once, before any test reads them (R-QA-09).
fn record() -> &'static Recorded {
    static RECORDED: std::sync::OnceLock<Recorded> = std::sync::OnceLock::new();
    RECORDED.get_or_init(|| {
        let recorded = record_all();
        if std::env::var_os(BLESS).is_some() {
            let committed = repo(FIXTURES);
            for path in tree(&committed).keys() {
                fs::remove_file(committed.join(path)).expect("stale fixture removed");
            }
            for (path, bytes) in &recorded.0 {
                fs::write(committed.join(path), bytes).expect("fixture written");
            }
        }
        recorded
    })
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
        let out = velme_command(env!("CARGO_BIN_EXE_velme"), env!("CARGO_TARGET_TMPDIR"))
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

/// The lock and the store of the project at `dir`, by path from `dir` with `/` separators: what an example directory
/// commits (D-141). The synth log and temporary files are each machine's own, and a missing lock is simply absent.
fn lock_and_store(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files: BTreeMap<String, Vec<u8>> = tree(&dir.join(".velme"))
        .into_iter()
        .filter(|(path, _)| path != "synth-log.jsonl" && !path.starts_with("tmp/"))
        .map(|(path, bytes)| (format!(".velme/{path}"), bytes))
        .collect();
    if let Ok(lock) = fs::read(dir.join("velme.lock")) {
        files.insert("velme.lock".to_owned(), lock);
    }
    files
}

/// Each example directory as building its examples one by one on the replay provider leaves it, from the fixtures
/// recorded now: the lock and the store, by the directory's path from the repository root.
fn build_example_dirs() -> BTreeMap<String, BTreeMap<String, Vec<u8>>> {
    let (fixtures, _) = record();
    let mut dirs: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (example, _) in EXAMPLES {
        let (dir, _) = example.rsplit_once('/').expect("an example in a directory");
        dirs.entry(dir).or_default().push(example);
    }
    let mut built = BTreeMap::new();
    for (name, examples) in dirs {
        let dir = scratch(&format!("{}-locked", name.replace('/', "-")));
        let fixture_dir = dir.join(FIXTURES);
        fs::create_dir_all(&fixture_dir).expect("fixture directory");
        for (path, bytes) in fixtures {
            fs::write(fixture_dir.join(path), bytes).expect("fixture written");
        }
        for example in examples {
            let file = format!("{}.velme", stem(example));
            fs::copy(repo(example), dir.join(&file)).expect("the example is copied");
            let out = velme(&dir, &["build", &file, "--provider", "replay"]);
            assert_eq!(out.code, 0, "{example}: {}{}", out.stdout, out.stderr);
        }
        built.insert(name.to_owned(), lock_and_store(&dir));
    }
    built
}

/// [`build_example_dirs`], once per test process; under `VELME_BLESS_FIXTURES=1` the example directories are rewritten
/// with it here, once, before any test reads them, so blessing then testing in place is one deterministic run (R-QA-09).
fn example_dirs() -> &'static BTreeMap<String, BTreeMap<String, Vec<u8>>> {
    static BUILT: std::sync::OnceLock<BTreeMap<String, BTreeMap<String, Vec<u8>>>> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| {
        let built = build_example_dirs();
        if std::env::var_os(BLESS).is_some() {
            for (name, files) in &built {
                let dir = repo(name);
                for path in lock_and_store(&dir).keys() {
                    fs::remove_file(dir.join(path)).expect("stale file removed");
                }
                for (path, bytes) in files {
                    let path = dir.join(path);
                    fs::create_dir_all(path.parent().expect("a parent")).expect("store directory");
                    fs::write(path, bytes).expect("file written");
                }
            }
        }
        built
    })
}

/// What a test that reads the committed fixtures or examples calls first: under `VELME_BLESS_FIXTURES=1` it waits for the
/// bless of [`record`] and [`example_dirs`]; otherwise it does nothing.
fn blessed() {
    if std::env::var_os(BLESS).is_some() {
        example_dirs();
    }
}

/// Each example directory commits the lock and the store that building its examples on the replay provider gives, so
/// every example runs with no API key; the manifests record the compiler version, so a version bump is re-blessed here
/// (D-141, `delivery/51` §2).
#[test]
fn the_committed_example_locks_and_artifacts_are_what_replay_builds() {
    for (name, files) in example_dirs() {
        assert!(
            lock_and_store(&repo(name)) == *files,
            "{name}: the lock or the store is out of date: run `{BLESS}=1 cargo test -p velme-cli --test examples` and \
             review the diff"
        );
    }
}

/// Examples-as-tests (`delivery/51` §2, D-141): every example passes `velme test --locked` where it is committed, run
/// from the repository root with no key, and leaves its directory as it was.
#[test]
fn every_committed_example_passes_velme_test_locked() {
    blessed();
    let root = repo("");
    for (example, _) in EXAMPLES {
        let dir = repo(example);
        let dir = dir.parent().expect("an example directory");
        let before = tree(dir);
        let out = velme(&root, &["test", "--locked", example]);
        assert_eq!(out.code, 0, "{example}: {}{}", out.stdout, out.stderr);
        assert!(!out.stdout.contains('✗'), "{example}: {}", out.stdout);
        assert!(
            tree(dir) == before,
            "{example}: `velme test --locked` wrote to {}",
            dir.display()
        );
    }
}

/// The README's quick start runs an example with no key and no build (D-142).
#[test]
fn the_readme_quick_start_runs_add_with_no_key() {
    blessed();
    let out = velme(
        &repo(""),
        &[
            "run",
            "examples/beginner/add.velme",
            "--goal",
            "Add",
            "--arg",
            "a=2",
            "--arg",
            "b=3",
        ],
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.ends_with("Result:\n5\n"), "{}", out.stdout);
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
    let committed = tree(&repo(FIXTURES));
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
    blessed();
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
