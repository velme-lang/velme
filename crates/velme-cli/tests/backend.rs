//! `--backend` end to end on the committed fixture projects (`tooling/40` §2, `runtime/31` R-SBX-02, R-SBX-18, D-117):
//! `run`, `trace` and `test` print byte-identical stdout and stderr and exit with the same code on `interp`, `wasm` and
//! `auto`, in human mode and under `--json`, for a success and for each deterministic failure. `--verbose` adds the
//! backend's notes on stderr and nothing else.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use velme_test_support::repo;

#[derive(Debug, PartialEq, Eq)]
struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("backend").join(name);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// Runs `velme` from the repository root with `stdin`, its user-level caches under `cache`, never the user's own
/// (R-SBX-20).
fn velme(cache: &Path, args: &[&str], stdin: &[u8]) -> Out {
    let mut child = Command::new(env!("CARGO_BIN_EXE_velme"))
        .args(args)
        .current_dir(repo(""))
        .env_remove("NO_COLOR")
        .env("XDG_CACHE_HOME", cache)
        .env("LOCALAPPDATA", cache)
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
    Out {
        stdout: String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(out.stderr).expect("stderr is UTF-8"),
        code: out.status.code().expect("velme exits with a code"),
    }
}

/// `out` with its `--json` document's timings, the fields ending in `_us` (`runtime/30` §8), taken out.
fn untimed(out: Out) -> Out {
    fn strip(json: &mut serde_json::Value) {
        match json {
            serde_json::Value::Object(map) => {
                map.retain(|key, _| !key.ends_with("_us"));
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut document: serde_json::Value = serde_json::from_str(&out.stdout).expect("one JSON document");
    strip(&mut document);
    Out {
        stdout: document.to_string(),
        ..out
    }
}

/// Every committed fixture project with a run of each of its goals worth making: its file, the goal, and the inputs,
/// as `--arg`s or as a document on stdin. Failures are among them: an overflow in a leaf, and an answer past
/// `max_output_bytes`.
fn cases() -> Vec<(&'static str, &'static str, Vec<String>, Vec<u8>)> {
    let arg = |a: &str| vec!["--arg".to_owned(), a.to_owned()];
    let args = |a: &[&str]| a.iter().flat_map(|a| arg(a)).collect::<Vec<_>>();
    let stdin = vec!["--input".to_owned(), "-".to_owned()];
    let player = r#"player={"name":"Lina","jump_height":3,"score":820}"#;
    let long = format!(r#"{{"name": "{}"}}"#, "a".repeat(1_100_000));
    vec![
        ("add/add.velme", "Add", args(&["a=2", "b=3"]), Vec::new()),
        ("add/add.velme", "Add", args(&["a=0.1", "b=0.2"]), Vec::new()),
        ("add_broken/add.velme", "Add", args(&["a=2", "b=3"]), Vec::new()),
        (
            "add_broken/add.velme",
            "Add",
            args(&["a=10000000000000000000", "b=10000000000000000000000"]),
            Vec::new(),
        ),
        (
            "double_then_add_one/double_then_add_one.velme",
            "Main",
            arg("x=4"),
            Vec::new(),
        ),
        (
            "double_then_add_one/double_then_add_one.velme",
            "Double",
            arg("x=-1.5"),
            Vec::new(),
        ),
        (
            "find_badge/find_badge.velme",
            "FindBadge",
            arg(r#"player={"name":"Lina","score":820}"#),
            Vec::new(),
        ),
        ("hello/hello.velme", "SayHello", arg(r#"name="Lina""#), Vec::new()),
        ("hello/hello.velme", "SayHello", stdin, long.into_bytes()),
        (
            "level_summary/level_summary.velme",
            "CreateLevelSummary",
            arg(r#"level={"enemy_count":12,"treasure_count":0,"base_score":9}"#),
            Vec::new(),
        ),
        (
            "order_total/order_total.velme",
            "OrderTotal",
            arg(r#"items=[{"price":10,"quantity":3},{"price":5,"quantity":1}]"#),
            Vec::new(),
        ),
        (
            "player_summary/player_summary.velme",
            "BuildPlayerSummary",
            arg(player),
            Vec::new(),
        ),
        (
            "player_summary/player_summary.velme",
            "FindBadge",
            arg(player),
            Vec::new(),
        ),
    ]
}

/// Each fixture's runs, and `test` of each fixture, give byte-identical stdout, stderr and exit code on `interp`,
/// `wasm` and `auto`: `run` and `trace`, human and `--json` (R-SBX-18, D-117; the M7 "user verifies").
#[test]
fn r_sbx_18_run_trace_and_test_are_byte_identical_on_every_backend() {
    let cache = scratch("identical");
    let mut failures = 0;
    for (file, goal, inputs, stdin) in cases() {
        let file = format!("tests/fixtures/run/{file}");
        for command in ["run", "trace"] {
            for json in [false, true] {
                let mut args: Vec<&str> = vec![command, &file, "--goal", goal];
                args.extend(inputs.iter().map(String::as_str));
                if json {
                    args.push("--json");
                }
                // Timings are no output of the program: `trace --json` has them, and they are left out (R-SBX-18).
                let timed = command == "trace" && json;
                let on = |backend: &str| {
                    let out = velme(&cache, &[&args[..], &["--backend", backend]].concat(), &stdin);
                    if timed { untimed(out) } else { out }
                };
                let interp = on("interp");
                failures += usize::from(interp.code != 0);
                assert_eq!(on("wasm"), interp, "{args:?} on wasm");
                assert_eq!(on("auto"), interp, "{args:?} on auto");
                // Without the flag it is the interpreter (D-117).
                let default = velme(&cache, &args, &stdin);
                assert_eq!(if timed { untimed(default) } else { default }, interp, "{args:?}");
            }
        }
    }
    assert!(failures >= 8, "{failures} failing runs");
    let mut files: Vec<&str> = cases().iter().map(|(file, ..)| *file).collect();
    files.dedup();
    for file in files {
        let path = format!("tests/fixtures/run/{file}");
        for json in [false, true] {
            let mut args = vec!["test", &path];
            if json {
                args.push("--json");
            }
            let on = |backend: &str| velme(&cache, &[&args[..], &["--backend", backend]].concat(), b"");
            let interp = on("interp");
            assert_eq!(on("wasm"), interp, "{args:?}");
            assert_eq!(on("auto"), interp, "{args:?}");
        }
    }
}

/// `--verbose` adds a note on stderr when the WASM backend turned its disk cache off, and changes nothing else: the
/// `--json` document is the interpreter's (R-SBX-12, R-SBX-20, D-120).
#[test]
fn r_sbx_12_verbose_notes_are_on_stderr_only() {
    let dir = scratch("verbose");
    // A cache base that is a file: the cache directory under it can't be made, so the disk cache is off.
    let cache = dir.join("cache");
    fs::write(&cache, "").expect("a file");
    let args = [
        "run",
        "tests/fixtures/run/add/add.velme",
        "--goal",
        "Add",
        "--arg",
        "a=2",
        "--arg",
        "b=3",
    ];
    let interp = velme(&cache, &[&args[..], &["--json"]].concat(), b"");
    let quiet = velme(&cache, &[&args[..], &["--json", "--backend", "wasm"]].concat(), b"");
    assert_eq!(quiet, interp);
    let loud = velme(
        &cache,
        &[&args[..], &["--json", "--backend", "wasm", "-v"]].concat(),
        b"",
    );
    assert_eq!((&loud.stdout, loud.code), (&interp.stdout, interp.code));
    assert!(
        loud.stderr.starts_with("note: the compiled-module cache in `"),
        "{}",
        loud.stderr
    );
    assert!(
        loud.stderr.ends_with("modules are compiled on every run\n"),
        "{}",
        loud.stderr
    );
    // The interpreter has no notes to give.
    let plain = velme(&cache, &[&args[..], &["--json", "-v"]].concat(), b"");
    assert_eq!(plain, interp);
}
