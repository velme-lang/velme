//! The workloads behind the `delivery/51` §6 targets (D-131), shared by the criterion benchmarks and the ignored
//! release-mode `ac_cmp_07_*` and `ac_qa_07_*` tests (D-130): a seeded program generator, the nested `reduce`, and the
//! fan-out of 128 trivial calls.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use velme_builtins::BUILTINS_VERSION;
use velme_builtins::limits::MAX_GOAL_CALLS;
use velme_ir::{IR_VERSION, calls};
use velme_sema::hir::Program;

use crate::ir_json::{binary, builtin, input, literal, local, number};
use crate::scrub::scrub_provider_env;
use crate::{goal_id, install, program};

/// A seeded sequence (splitmix64): the same numbers on every run and platform.
struct Seeded(u64);

impl Seeded {
    /// The next number below `bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) % bound
    }
}

/// The declarations of a generated program, one at a time: record types, number leaves with checks and examples, text
/// leaves over a record, and wired goals calling two number leaves, so no goal's call tree holds more than three goals.
struct Generator {
    rng: Seeded,
    next: usize,
    types: Vec<usize>,
    leaves: Vec<usize>,
}

impl Generator {
    fn new(seed: u64) -> Generator {
        Generator {
            rng: Seeded(seed),
            next: 0,
            types: Vec::new(),
            leaves: Vec::new(),
        }
    }

    /// The next declaration, ending in a blank line, and whether it is a goal.
    fn declaration(&mut self) -> (String, bool) {
        let i = self.next;
        self.next += 1;
        let (k, m) = (self.rng.below(90) + 2, self.rng.below(1000));
        let mut text = String::new();
        let goal = match self.rng.below(4) {
            0 => {
                self.types.push(i);
                let _ = write!(
                    text,
                    "type T{i}:\n    name: Text\n    score: Number\n    level: Number\n\n"
                );
                false
            }
            2 if !self.types.is_empty() => {
                let t = self.types[usize::try_from(self.rng.below(self.types.len() as u64)).expect("fits")];
                let _ = write!(
                    text,
                    "goal L{i}(r: T{t}) -> Text:\n    plan: |\n        Rate the record High for a score of at least {k},\n        \
                     Low otherwise.\n    check:\n        - result == \"High\" or result == \"Low\"\n    examples:\n        \
                     - L{i}(T{t}(name: \"n{i}\", score: {k}, level: 1)) == \"High\"\n        \
                     - L{i}(T{t}(name: \"m{i}\", score: 1, level: {m})) == \"Low\"\n\n"
                );
                true
            }
            3 if self.leaves.len() >= 2 => {
                let mut pick = || self.leaves[usize::try_from(self.rng.below(self.leaves.len() as u64)).expect("fits")];
                let (a, b) = (pick(), pick());
                let _ = write!(
                    text,
                    "# Calls two leaves, the second on the first's answer.\ngoal W{i}(x: Number) -> Number:\n    call:\n        \
                     p = G{a}(x, {k})\n        q = G{b}(p, x)\n    plan: \"Add p and q.\"\n    check:\n        \
                     - result == p + q\n\n"
                );
                true
            }
            _ => {
                self.leaves.push(i);
                let _ = write!(
                    text,
                    "goal G{i}(a: Number, b: Number) -> Number:\n    plan: \"Multiply a by {k}, then add b.\"\n    check:\n        \
                     - result == a * {k} + b\n    examples:\n        - G{i}(1, 2) == {}\n        - G{i}(0, {m}) == {m}\n\n",
                    k + 2
                );
                true
            }
        };
        (text, goal)
    }
}

/// The first line of every generated program.
const HEADER: &str = "language: velme/0.1\n\n";

/// A checked program of exactly `lines` lines from the generator seeded with `seed` (D-131): declarations while the
/// next one fits, then comment lines.
pub fn source(seed: u64, lines: usize) -> String {
    let mut generator = Generator::new(seed);
    let mut text = HEADER.to_owned();
    loop {
        let (next, _) = generator.declaration();
        if text.lines().count() + next.lines().count() > lines {
            break;
        }
        text.push_str(&next);
    }
    while text.lines().count() < lines {
        text.push_str("# padding\n");
    }
    text
}

/// A checked program of exactly `goals` goals from the generator seeded with `seed`, and its record types (D-131).
pub fn goals(seed: u64, goals: usize) -> String {
    let mut generator = Generator::new(seed);
    let (mut text, mut made) = (HEADER.to_owned(), 0);
    while made < goals {
        let (next, goal) = generator.declaration();
        text.push_str(&next);
        made += usize::from(goal);
    }
    text
}

/// A `reduce` over `range(n)` nested inside a `reduce` over `range(n)`, adding one each time: `n * n` additions, and
/// that number as its answer. With `n` 1000, the `delivery/51` §6 workload (D-131).
pub fn nested_reduce(n: i64) -> Json {
    let range = builtin("range", &[literal(n)]);
    let inner = json!({"kind": "reduce", "list": range, "init": local("outer"),
        "fn": {"acc": "sum", "param": "j", "body": binary("add", local("sum"), literal(1))}});
    json!({"kind": "reduce", "list": range, "init": literal(0), "fn": {"acc": "outer", "param": "i", "body": inner}})
}

/// The file of the [`fan_out`] project.
pub const FAN_FILE: &str = "fan.velme";

/// `Fan` calls `Id` 127 times, so a run of it runs [`MAX_GOAL_CALLS`] goals, counting itself (D-131).
pub fn fan_source() -> String {
    let children = MAX_GOAL_CALLS - 1;
    let mut text = format!(
        "{HEADER}goal Id(x: Number) -> Number:\n    plan: \"Return x.\"\n\ngoal Fan(x: Number) -> Number:\n    call:\n"
    );
    for c in 0..children {
        let _ = writeln!(text, "        c{c} = Id(x)");
    }
    text.push_str("    plan: \"Return c0.\"\n");
    text
}

/// The IR of goal `name` of [`fan_source`]: `Id` returns its input, `Fan` its first child's answer.
pub fn fan_ir(program: &Program, name: &str) -> String {
    let body = if name == "Id" { input("x") } else { local("c0") };
    let calls = calls(program, goal_id(program, name)).expect("calls");
    json!({"ir_version": IR_VERSION, "builtins_version": BUILTINS_VERSION, "goal": name, "types": {},
           "inputs": [["x", number()]], "output": number(), "calls": calls, "body": body})
    .to_string()
}

/// A fresh project at `dir` with both goals of [`fan_source`] installed, and its checked program.
pub fn fan_out(dir: &Path) -> (PathBuf, Program) {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", dir.display()),
        _ => {}
    }
    fs::create_dir_all(dir).expect("project directory");
    let source = fan_source();
    let program = program(&source);
    fs::write(dir.join(FAN_FILE), &source).expect("source written");
    for goal in ["Id", "Fan"] {
        install(dir, FAN_FILE, &program, &fan_ir(&program, goal));
    }
    (dir.to_path_buf(), program)
}

/// The shortest of `runs` timings of `run`, after one run to warm up: what each `delivery/51` §6 target is held to
/// (D-130).
pub fn best_of(runs: usize, mut run: impl FnMut()) -> Duration {
    run();
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .min()
        .expect("at least one run")
}

/// The ratios `b / a` of `pairs` paired timings of `a` and `b`, sorted, after two runs of each to warm up. Each pair runs
/// back to back, the first going first in even pairs and second in odd ones, so drift in the machine's state (clock,
/// heap, caches) falls on both alike: what the `delivery/51` §6 WASM row is held to, by the median (D-130).
pub fn paired_ratios(pairs: usize, a: impl FnMut(), b: impl FnMut()) -> Vec<f64> {
    let mut ratios: Vec<f64> = paired(pairs, a, b).into_iter().map(|(ta, tb)| tb / ta).collect();
    ratios.sort_by(f64::total_cmp);
    ratios
}

/// The timings in seconds of `a` and `b` in `pairs` pairs, in the order run, after two runs of each to warm up; each pair
/// runs back to back, the first going first in even pairs and second in odd ones, as [`paired_ratios`] says.
pub fn paired(pairs: usize, mut a: impl FnMut(), mut b: impl FnMut()) -> Vec<(f64, f64)> {
    for _ in 0..2 {
        a();
        b();
    }
    let time = |run: &mut dyn FnMut()| {
        let start = Instant::now();
        run();
        start.elapsed().as_secs_f64()
    };
    (0..pairs)
        .map(|i| {
            if i % 2 == 0 {
                let ta = time(&mut a);
                (ta, time(&mut b))
            } else {
                let tb = time(&mut b);
                (time(&mut a), tb)
            }
        })
        .collect()
}

/// Fails unless this is a release build: the targets are for one (`delivery/51` §6), and a debug build would miss them.
pub fn assert_release(test: &str) {
    if cfg!(debug_assertions) {
        panic!("{test} measures a release build: run `cargo xtask gate`, or add --release");
    }
}

/// The `find_badge` example's project, with its lock and artifact (D-131), copied to a fresh `dir`: the input of the
/// `velme run --locked` target, with an empty [`user_cache`].
pub fn find_badge(dir: &Path) -> PathBuf {
    fixture_project("find_badge", dir)
}

/// The committed project `tests/fixtures/run/<name>`, one example with its lock and artifacts, copied to a fresh `dir`
/// with an empty [`user_cache`]: `name.velme` is its file.
pub fn fixture_project(name: &str, dir: &Path) -> PathBuf {
    for old in [dir.to_path_buf(), user_cache(dir)] {
        match fs::remove_dir_all(&old) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => panic!("{}: {e}", old.display()),
            _ => {}
        }
    }
    let fixture = crate::repo(&format!("tests/fixtures/run/{name}"));
    let artifacts = Path::new(".velme").join("artifacts");
    fs::create_dir_all(dir.join(&artifacts)).expect("project directory");
    for file in [format!("{name}.velme"), "velme.lock".to_owned()] {
        fs::copy(fixture.join(&file), dir.join(&file)).expect("copied");
    }
    for entry in fs::read_dir(fixture.join(&artifacts)).expect("artifacts") {
        let path = entry.expect("entry").path();
        fs::copy(&path, dir.join(&artifacts).join(path.file_name().expect("a name"))).expect("copied");
    }
    dir.to_path_buf()
}

/// The arguments of the timed `velme run --locked` of [`find_badge`] on `backend`.
pub fn find_badge_run(backend: &str) -> Vec<String> {
    let args = [
        "run",
        "find_badge.velme",
        "--goal",
        "FindBadge",
        "--arg",
        r#"player={"name":"Lina","score":820}"#,
        "--locked",
        "--backend",
        backend,
    ];
    args.map(str::to_owned).to_vec()
}

/// The user-level cache of the project at `dir`, beside it rather than in it, since the WASM module cache refuses a
/// directory inside the project (R-SBX-20): `$XDG_CACHE_HOME` and `%LOCALAPPDATA%` for [`velme`].
pub fn user_cache(dir: &Path) -> PathBuf {
    dir.with_extension("cache")
}

/// The `velme` binary `exe` run in `dir` with `args`, no provider setting from the environment, its user-level config
/// under `dir/home` and its caches at [`user_cache`] (AC-QA-02, R-SBX-20); it must exit 0.
pub fn velme(exe: &Path, dir: &Path, args: &[String]) {
    velme_stderr(exe, dir, args);
}

/// [`velme`], giving back what the run printed on stderr.
pub fn velme_stderr(exe: &Path, dir: &Path, args: &[String]) -> String {
    let (home, cache) = (dir.join("home"), user_cache(dir));
    fs::create_dir_all(&home).expect("home directory");
    let mut command = std::process::Command::new(exe);
    command.args(args).current_dir(dir);
    scrub_provider_env(&mut command, &home);
    let out = command
        .env("XDG_CACHE_HOME", &cache)
        .env("LOCALAPPDATA", &cache)
        .output()
        .expect("velme runs");
    assert!(
        out.status.success(),
        "velme {args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}
