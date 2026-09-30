//! `velme` binary: the only crate that prints, reads the environment and picks exit codes (R-CMP-03).
#![forbid(unsafe_code)]

mod args;
mod config;
mod project;
mod synth;

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use velme_builtins::Value;
use velme_diagnostics::render::{self, JsonDiagnostic, LineIndex};
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_runtime::{
    ARTIFACT_FORMAT, BuildInput, CallStatus, EntryError, GoalRun, Lock, LockedGoal, Mode, Options, OrderedValue,
    Registry, Source, Status, Store, Trace, decode_inputs, explain, find_goal, load, run_goal, test_goal,
};
use velme_sema::hir::{GoalId, Program};
use velme_syntax::SourceFile;
use velme_synth::{SynthBackend, SynthOptions};

use crate::args::{Bad, Cli, Cmd, Color, Parsed};
use crate::config::Settings;
use crate::project::Project;
use crate::synth::{BuildFlags, NotConfigured, backend, provider_name, reword_not_configured, synth_options};

/// Exit codes (`tooling/40` §4, R-CLI-10).
const EXIT_OK: u8 = 0;
const EXIT_INVALID_PROGRAM: u8 = 1;
const EXIT_BUILD_FAILED: u8 = 2;
const EXIT_EXECUTION_FAILED: u8 = 3;
const EXIT_ARTIFACT: u8 = 4;
const EXIT_USAGE: u8 = 64;
const EXIT_INTERNAL: u8 = 70;

/// When codes from different rows fail together, the earlier exit code here wins (R-CLI-16).
const EXIT_PRECEDENCE: [u8; 6] = [
    EXIT_INTERNAL,
    EXIT_USAGE,
    EXIT_INVALID_PROGRAM,
    EXIT_ARTIFACT,
    EXIT_BUILD_FAILED,
    EXIT_EXECUTION_FAILED,
];

/// The `--json` envelope version (D-49, R-CLI-15).
const JSON_FORMAT: &str = "velme-cli/1";

/// Stack for the threads commands and analysis run on. The D-71 and `compiler/21` §7 limits bound how deep parsing,
/// checking, validation, evaluation and value rendering recurse (`runtime/30` §3); the main thread's default stack
/// differs by platform (1 MiB on Windows), so the size is set here (R-SYN-19).
const STACK: usize = 64 * 1024 * 1024;

/// The `velme check` progress lines (`tooling/40` §3.3) this build can reach, each with the code prefixes of the phase
/// it reports on (`reference/90` groups codes by phase). The static call limits are checked with the call graph
/// (`runtime/30` §7).
const CHECK_LINES: [(&str, &[&str]); 3] = [
    ("✓ Parsed", &["VL01"]),
    ("✓ Types valid", &["VL02"]),
    ("✓ Call graph valid", &["VL03", "VL0605"]),
];

const USAGE: &str = "usage: velme check FILE [--json]\n       \
                     velme build FILE [--provider NAME] [--model ID] [--external-url URL] [--ollama-url URL] [--locked] [--offline] [-v] [--json]\n       \
                     velme run FILE --goal G [--input FILE.json|-] [--arg NAME=JSON]... [--jobs N] [--build] [--json]\n       \
                     velme test FILE [--goal G] [--jobs N] [--build] [--json]\n       \
                     velme explain FILE --goal G [--json]\n       \
                     velme artifact FILE --goal G [--json]\n       \
                     velme trace FILE --goal G [--input FILE.json|-] [--arg NAME=JSON]... [--jobs N] [--build] [--json]\n       \
                     velme gc | velme cache clean\n       \
                     velme --version\n\
                     every command also takes [--color auto|always|never] [-q] [-v]; FILE commands take [--config PATH]";

/// The `velme check` line for locked IR (`tooling/40` §3.3), shown when the project has a lock.
const IR_LINE: &str = "✓ IR valid        ";

/// The last `velme check` line (`tooling/40` §3.3): the file's checks and examples are well-formed.
const CHECKS_LINE: &str = "✓ Checks valid\n";

/// `--input`'s name for standard input (`tooling/40` §3.1).
const STDIN: &str = "-";

/// How standard input is named when it can't be read.
const STDIN_NAME: &str = "standard input";

fn version_line() -> String {
    format!("velme {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = on_big_stack(|| command(&args)).unwrap_or_else(|| stopped(&args));
    ExitCode::from(code)
}

/// Reports the bug that stopped the command `args` name, and gives its exit code: under `--json` as the envelope, on
/// standard output like any other outcome (R-CLI-15), otherwise without source lines, as it belongs to no place in any
/// file.
fn stopped(args: &[String]) -> u8 {
    let file = match args::parse(args) {
        Ok(Parsed::Run(cli)) if cli.json => cli.file,
        _ => {
            print_err(&format!("{}\n", Diagnostic::internal_error().message));
            return EXIT_INTERNAL;
        }
    };
    let shown = if file.is_empty() {
        String::new()
    } else {
        shown_path(&Project::of(Path::new(&file)), &file)
    };
    if let Some(out) = internal_envelope(&shown) {
        print_out(&out);
    }
    EXIT_INTERNAL
}

/// The `--json` envelope of a command on the file shown as `path` that a bug stopped: `VL0607` alone.
fn internal_envelope(path: &str) -> Option<String> {
    diagnostic_envelope(Diagnostic::internal_error(), path)
}

/// The `--json` envelope of a command that only has `diagnostic` to report, on the file shown as `path`.
fn diagnostic_envelope(diagnostic: Diagnostic, path: &str) -> Option<String> {
    let envelope = Envelope {
        format: JSON_FORMAT,
        status: "failed",
        results: Vec::new(),
        diagnostics: vec![JsonDiagnostic::new(&diagnostic, path, &LineIndex::new(""))],
        notices: Vec::new(),
        summary: None,
    };
    let out = serde_json::to_string_pretty(&envelope).ok()?;
    Some(format!("{}\n", render::escape_json(&out)))
}

/// Runs the command `args` name, and gives its exit code.
fn command(args: &[String]) -> u8 {
    match args::parse(args) {
        Ok(Parsed::Version) => {
            print_out(&format!("{}\n", version_line()));
            EXIT_OK
        }
        Ok(Parsed::Run(cli)) => match cli.cmd {
            Cmd::Check => check(&cli),
            Cmd::Build => build_command(&cli),
            Cmd::Run => run(&cli, false),
            Cmd::Trace => run(&cli, true),
            Cmd::Explain => explain_goal(&cli),
            Cmd::Artifact => artifact(&cli),
            Cmd::Test => test(&cli),
            Cmd::Gc => gc(&cli),
            Cmd::CacheClean => cache_clean(&cli),
        },
        Err(bad) => usage_error(&bad),
    }
}

/// A command line that is wrong (R-CLI-14, R-CLI-21, D-108): `VL0902`, in the envelope under `--json`, else the usage and the
/// diagnostic; exit 64 either way.
fn usage_error(bad: &Bad) -> u8 {
    let path = bad
        .file
        .as_deref()
        .map(|file| shown_path(&Project::of(Path::new(file)), file))
        .unwrap_or_default();
    if bad.json {
        // A bad command line is about no file (D-111).
        match diagnostic_envelope(bad.diagnostic.clone(), "") {
            Some(out) => print_out(&out),
            None => return EXIT_INTERNAL,
        }
    } else {
        let color = stream_colors(bad.color, false, std::io::stderr().is_terminal(), no_color()).1;
        print_err(&format!("{USAGE}\n\n"));
        print_err(&render::render_human(
            std::slice::from_ref(&bad.diagnostic),
            &path,
            None,
            color,
        ));
    }
    exit_code(std::slice::from_ref(&bad.diagnostic))
}

/// Output errors (a closed pipe) are ignored: there is nowhere left to report them.
fn print_out(s: &str) {
    let _ = std::io::stdout().lock().write_all(s.as_bytes());
}

fn print_err(s: &str) {
    let _ = std::io::stderr().lock().write_all(s.as_bytes());
}

/// A source file read and analyzed (phases 1–6, `compiler/20` §3).
struct Analyzed {
    /// The path shown: from the project root (R-CLI-19), or as given if the file can't be found.
    path: String,
    /// The file's project, if the file exists.
    project: Option<Project>,
    /// Its text, if it could be read.
    text: Option<String>,
    /// The checked program, if it has no errors.
    program: Option<Program>,
    /// Everything analysis found.
    diagnostics: Vec<Diagnostic>,
    /// The project's and the user's config, each validated, or what is wrong with them. Read before the file is analyzed,
    /// since a `[budget]` in it sets the limits the file's goals are checked against (D-8).
    settings: Result<Settings, Vec<Diagnostic>>,
}

/// How the file `arg` is shown: from its project's root (R-CLI-19), or as given if it can't be found.
fn shown_path(project: &std::io::Result<Project>, arg: &str) -> String {
    project.as_ref().map_or_else(|_| display_path(arg), |p| p.file.clone())
}

fn analyze(cli: &Cli) -> Analyzed {
    let arg = cli.file.as_str();
    let project = Project::of(Path::new(arg));
    let settings = match &project {
        Ok(project) => config::load(&project.root, cli.config.as_deref()),
        Err(_) => Ok(Settings::default()),
    };
    let defaults = settings
        .as_ref()
        .map_or(velme_sema::hir::Budget::SYSTEM, |s| s.project.budget.effective());
    let path = shown_path(&project, arg);
    let read = std::fs::read(arg).and_then(|bytes| project.as_ref().map(|_| bytes).map_err(clone_error));
    let (text, program, diagnostics) = match read {
        Err(err) => (None, None, vec![SourceFile::unreadable(&path, &err)]),
        Ok(bytes) => match SourceFile::from_bytes(path.clone(), bytes.clone()) {
            // A thread of its own, so a bug in analysis is reported like any other (R-SYN-19).
            Ok(file) => match on_big_stack(|| velme_sema::analyze_with(&file, defaults)) {
                Some((program, diagnostics)) => (Some(file.text), program, diagnostics),
                // Shown without source lines: it belongs to no place in the file.
                None => (None, None, vec![Diagnostic::internal_error()]),
            },
            // The span is a byte offset into the raw bytes, which the lossy text keeps up to the bad byte.
            Err(diag) => (Some(String::from_utf8_lossy(&bytes).into_owned()), None, vec![diag]),
        },
    };
    let program = program.filter(|_| !diagnostics.iter().any(Diagnostic::is_error));
    Analyzed {
        path,
        project: project.ok(),
        text,
        program,
        diagnostics,
        settings,
    }
}

/// What a command found, to print as text or as the `--json` envelope (`tooling/40` §3.2).
#[derive(Default)]
struct Outcome {
    /// Printed to stdout in human mode, above the diagnostics: the lines that tell how it went, which `-q` drops (R-CLI-24).
    progress: String,
    /// What the command was asked for, printed to stdout in human mode after `progress`: a result, an explanation, an
    /// artifact. `-q` keeps it.
    shown: String,
    /// Whether `shown` is already escaped (R-CLI-17), as `velme artifact` does so its IR stays JSON.
    escaped: bool,
    /// The notice lines of `tooling/41` R-SEC-12, which `--json` carries instead of stderr.
    notices: Vec<String>,
    /// The build summary of `compiler/22` R-SYNTH-21, for `--json`.
    summary: Option<serde_json::Value>,
    /// The diagnostics of the file rather than of one goal (D-72), besides the analysis's own.
    diagnostics: Vec<Diagnostic>,
    /// One per goal the command reports on.
    results: Vec<GoalResult>,
}

/// What a command found for one goal (R-CLI-15).
struct GoalResult {
    goal: String,
    status: &'static str,
    diagnostics: Vec<Diagnostic>,
    result: Option<Value>,
    /// The run and the file it is of, for `velme trace` to show its trace (`runtime/30` §8).
    trace: Option<(GoalRun, String)>,
    /// The artifact `velme artifact` shows, as the `artifact` of `--json` (R-CLI-22, D-111).
    artifact: Option<LockedGoal>,
    /// The goals it called, in source order (R-CLI-27).
    calls: Vec<CallJson>,
}

impl GoalResult {
    fn new(goal: &str, outcome: Result<Option<Value>, Vec<Diagnostic>>) -> Self {
        let (status, diagnostics, result) = match outcome {
            Ok(result) => ("ok", Vec::new(), result),
            Err(diagnostics) => ("failed", diagnostics, None),
        };
        GoalResult {
            goal: goal.to_owned(),
            status,
            diagnostics,
            result,
            trace: None,
            artifact: None,
            calls: Vec::new(),
        }
    }

    /// A result with a status of its own, and no value.
    fn with_status(goal: &str, status: &'static str, diagnostics: Vec<Diagnostic>) -> Self {
        GoalResult {
            status,
            diagnostics,
            ..GoalResult::new(goal, Ok(None))
        }
    }
}

/// The settings of a command on the analyzed file: its project's `velme.toml` or the `--config` file, and the user-level file,
/// each validated (R-CLI-25). A file that isn't there has no project, and no settings to be wrong.
fn settings(analyzed: &Analyzed) -> Result<Settings, Box<Outcome>> {
    analyzed.settings.clone().map_err(|d| Box::new(Outcome::with(d)))
}

/// `velme check FILE`: phases 1–6 (`compiler/20` §3), then the locked IR of the file's goals if the project has a
/// lock (`tooling/40` §2). Check evaluation follows in a later phase.
fn check(cli: &Cli) -> u8 {
    let analyzed = analyze(cli);
    let mut outcome = Outcome::file(&analyzed);
    if let Err(bad) = settings(&analyzed) {
        return finish(&analyzed, &bad, cli);
    }
    if let (Some(program), Some(project)) = (&analyzed.program, &analyzed.project) {
        match locked_ir(project, program) {
            Ok(None) => {}
            Ok(Some((locked, failed))) if failed.is_empty() => {
                let s = if locked == 1 { "" } else { "s" };
                outcome
                    .progress
                    .push_str(&format!("{IR_LINE}({locked} goal{s} locked)\n"));
            }
            Ok(Some((_, failed))) => outcome.results = failed,
            Err(diag) => outcome.diagnostics.push(diag),
        }
        // Its checks and examples were read with everything else in phases 1–6, and nothing above failed.
        if outcome.results.is_empty() && outcome.diagnostics.is_empty() {
            outcome.progress.push_str(CHECKS_LINE);
        }
    }
    finish(&analyzed, &outcome, cli)
}

/// Validates the locked IR of every goal of `program` pinned in its project's lock (R-ART-10): `None` without a lock,
/// else how many loaded and the goals whose locked IR is unusable. An entry stale only because the source changed
/// since the last build is left to `velme build`; it isn't locked IR of this source.
fn locked_ir(project: &Project, program: &Program) -> Result<Option<(usize, Vec<GoalResult>)>, Diagnostic> {
    let Some(lock) = Lock::read(&project.root).map_err(|e| e.diagnostic())? else {
        return Ok(None);
    };
    let store = Store::new(&project.root);
    let (mut locked, mut failed) = (0, Vec::new());
    for (i, goal) in program.goals.iter().enumerate() {
        if lock.entry(&project.file, &goal.name).is_none() {
            continue;
        }
        match load(program, GoalId(i), &project.file, &lock, &store) {
            Ok(_) => locked += 1,
            Err(EntryError::Stale(causes)) if causes.iter().all(|c| c.is_source_change()) => {}
            Err(error) => failed.push(GoalResult::new(
                &goal.name,
                Err(vec![error.diagnostic(&goal.name, goal.span)]),
            )),
        }
    }
    Ok(Some((locked, failed)))
}

/// `velme build FILE`: checks the file, then gives every goal without a fresh lock entry a verified artifact and a lock
/// entry (`tooling/40` §2, `runtime/32` R-ART-15).
fn build_command(cli: &Cli) -> u8 {
    let analyzed = analyze(cli);
    let settings = match settings(&analyzed) {
        Ok(settings) => settings,
        Err(bad) => return finish(&analyzed, &bad, cli),
    };
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), cli);
    };
    let outcome = build_phase(cli, &analyzed, (program, text, project), &settings);
    finish(&analyzed, &outcome, cli)
}

/// Whether a build outcome stops a `--build` command before it runs anything: a goal that isn't built, or an error.
fn build_stopped(built: &Outcome) -> bool {
    built.results.iter().any(|r| r.status != "ok") || built.diagnostics.iter().any(Diagnostic::is_error)
}

/// The build of `velme build` and of `--build` on `run`, `test` and `trace` (D-28): every goal without a fresh lock entry gets
/// a verified artifact and a lock entry. With `--locked` or `--offline` no provider is constructed. With `-v`, every attempt
/// of a failed goal is listed (`compiler/22` R-SYNTH-13).
fn build_phase(cli: &Cli, analyzed: &Analyzed, file: (&Program, &str, &Project), settings: &Settings) -> Outcome {
    let (program, text, project) = file;
    let json = cli.json;
    let flags = BuildFlags {
        provider: cli.provider.as_deref(),
        model: cli.model.as_deref(),
        external_url: cli.external_url.as_deref(),
        ollama_url: cli.ollama_url.as_deref(),
        settings,
        verbose: cli.verbose,
        locked: cli.locked,
        offline: cli.offline,
    };
    let name = provider_name(&flags);
    let verbose = flags.verbose;
    // `--locked` is checked first, so with `--offline` too a stale goal is `VL0702` (R-CLI-05, D-106).
    let mode = if flags.locked {
        Mode::Locked
    } else if flags.offline {
        Mode::Offline
    } else {
        Mode::Build
    };
    // Neither flag constructs a provider, not even to ask its identity (R-CLI-04, R-CLI-05, AC-SYNTH-08).
    let contacts = mode == Mode::Build;
    // One set of options for the prompt side and the loop, so they can't disagree (R-SYNTH-40).
    let (options, clamped) = if contacts {
        synth_options(name, project, settings)
    } else {
        (SynthOptions::default(), Vec::new())
    };
    // The names and values are still checked, without building anything or reading a key (R-CLI-21).
    if !contacts && let Err(diagnostic) = synth::check_flags(name, &flags) {
        return Outcome::with(vec![diagnostic]);
    }
    let chosen = contacts.then(|| backend(name, project, &flags, &options));
    // A provider that can't be used doesn't stop a build that needs none; a name or script that is wrong does.
    if let Some(Err(diagnostic)) = &chosen
        && matches!(diagnostic.code, Code::InvalidInput | Code::FileError)
    {
        return Outcome::with(vec![diagnostic.clone()]);
    }
    let mut notices = Vec::new();
    let mut announce = |line: &str| {
        if json && !line.is_empty() {
            notices.push(line.to_owned());
        } else if !line.is_empty() {
            // The line names the user's external host and model, text Velme didn't produce (`tooling/41` R-SEC-12, D-47).
            print_err(&format!("{}\n", render::escape(line)));
        }
    };
    // Why a request would end with `VL0405` in the words of the setup: a provider that couldn't be built, or an Anthropic
    // one built with no usable key, which still answers from the store and sends nothing (R-SYNTH-02).
    let key_problem = if contacts && name == "anthropic" {
        synth::key_problem()
    } else {
        None
    };
    let unusable = chosen.as_ref().and_then(|c| c.as_ref().err()).or(key_problem.as_ref());
    let notice = match &chosen {
        Some(Ok((_, notice))) if key_problem.is_none() && !clamped.is_empty() => {
            format!("{notice} {}", clamped.join(" "))
        }
        Some(Ok((_, notice))) if key_problem.is_none() => notice.clone(),
        _ => String::new(),
    };
    let backend: Option<&dyn SynthBackend> = match &chosen {
        None => None,
        Some(Ok((backend, _))) => Some(backend.as_ref()),
        Some(Err(_)) => Some(&NotConfigured),
    };
    let input = BuildInput {
        mode,
        program,
        source: text,
        project: &project.root,
        file: &project.file,
        backend,
        options,
        run: Options::default(),
    };
    let report = velme_runtime::build(&input, &mut || announce(&notice));
    let model = synth::model_name(flags.model, settings);
    let mut outcome = Outcome::file(analyzed);
    outcome.progress.clear();
    for goal in &report.goals {
        let (line, status) = match goal.status {
            Status::Built(source) => (built_line(source), "ok"),
            Status::Failed => ("✗", "failed"),
            Status::Pending => ("… waiting for an answer", "pending"),
            Status::Blocked => ("– not built", "blocked"),
        };
        outcome.progress.push_str(&format!("{}  {line}\n", goal.goal));
        let mut diagnostics = goal.diagnostics.clone();
        reword_not_configured(&mut diagnostics, name, &model, unusable, &goal.goal);
        // The checked-again note stays; only the per-attempt lines wait for `-v` (R-SYNTH-13, R-SYNTH-46).
        let mut notes = goal.notes.clone();
        if verbose {
            notes.extend(goal.attempts.iter().cloned());
        }
        if let Some(first) = diagnostics.first_mut() {
            first.notes.extend(notes);
        } else {
            for note in notes {
                outcome.progress.push_str(&format!("  note: {note}\n"));
            }
        }
        outcome
            .results
            .push(GoalResult::with_status(&goal.goal, status, diagnostics));
    }
    outcome.diagnostics.extend(report.diagnostics.iter().cloned());
    let summary = report.summary;
    outcome.progress.push_str(&format!(
        "\n{} provider calls, {} tokens in, {} out; {} goals up to date, {} from the store\n",
        summary.calls, summary.usage.input_tokens, summary.usage.output_tokens, summary.lock_hits, summary.store_hits
    ));
    outcome.summary = Some(serde_json::json!({
        "calls": summary.calls,
        "input_tokens": summary.usage.input_tokens,
        "output_tokens": summary.usage.output_tokens,
        "cache_read_tokens": summary.usage.cache_read_tokens,
        "cache_write_tokens": summary.usage.cache_write_tokens,
        "lock_hits": summary.lock_hits,
        "store_hits": summary.store_hits,
        "synthesized": summary.synthesized,
    }));
    outcome.notices = notices;
    outcome
}

/// What the progress line of a built goal says.
fn built_line(source: Source) -> &'static str {
    match source {
        Source::Lock | Source::Reverified => "✓ up to date",
        Source::Store => "✓ from the store",
        Source::Synthesized => "✓ built",
        Source::Compiler => "✓ built by the compiler",
    }
}

/// `velme run FILE --goal G`: runs a goal with its locked artifact and those of the goals it calls on the reference
/// interpreter, its calls wave by wave, then its checks (`tooling/40` §2, `runtime/30` §4). `--jobs` sets the workers; with
/// `--build` the file is built first (D-28). With `traced` (`velme trace`) the whole execution trace is printed too
/// (`runtime/30` §8).
fn run(cli: &Cli, traced: bool) -> u8 {
    let analyzed = analyze(cli);
    let settings = match settings(&analyzed) {
        Ok(settings) => settings,
        Err(bad) => return finish(&analyzed, &bad, cli),
    };
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), cli);
    };
    let goal = cli.goal.as_deref().unwrap_or_default();
    let id = match find_goal(program, goal) {
        Ok(id) => id,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), cli),
    };
    let input = match cli.input.as_deref().map(read_input).transpose() {
        Ok(input) => input,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), cli),
    };
    let inputs = decode_inputs(program, id, input.as_deref(), &cli.args);
    // A typo in an argument costs no build, and so no provider call.
    if let (true, Err(diagnostics)) = (cli.build, &inputs) {
        return finish(&analyzed, &Outcome::with(diagnostics.clone()), cli);
    }
    let built = cli
        .build
        .then(|| build_phase(cli, &analyzed, (program, text, project), &settings));
    if let Some(built) = built.as_ref().filter(|b| build_stopped(b)) {
        return finish(&analyzed, built, cli);
    }
    // Input and lock problems are both reported, so the exit code follows R-CLI-16's precedence.
    let registry = registry(project, program, id);
    let options = cli
        .jobs
        .map_or_else(Options::default, |jobs| Options::default().with_jobs(jobs));
    let (executed, result) = match (inputs, registry) {
        (Ok(inputs), Ok(registry)) => {
            let executed = run_goal(program, id, text, &registry, inputs, options);
            let result = executed.result().map(Some);
            (Some(executed), result)
        }
        (inputs, registry) => (
            None,
            Err(inputs
                .err()
                .into_iter()
                .flatten()
                .chain(registry.err().into_iter().flatten())
                .collect()),
        ),
    };
    let mut progress = match &executed {
        Some(executed) if !traced => call_lines(executed),
        Some(_) => String::new(),
        None => format!("{goal}  ✗\n"),
    };
    let mut shown = match &executed {
        Some(executed) if traced => executed.trace_text(),
        _ => String::new(),
    };
    if let Ok(Some(value)) = &result {
        // The blank line sits between the lines before it and the value.
        if traced {
            shown.push('\n');
        } else {
            progress.push('\n');
        }
        shown.push_str(&format!("Result:\n{}\n", pretty(value)));
    }
    let mut goal_result = GoalResult::new(goal, result);
    if let Some(executed) = &executed {
        goal_result.calls = executed.calls.iter().map(CallJson::of).collect();
    }
    if traced {
        goal_result.trace = executed.map(|executed| (executed, analyzed.path.clone()));
    }
    let mut outcome = Outcome {
        progress,
        shown,
        diagnostics: Vec::new(),
        results: vec![goal_result],
        ..Outcome::default()
    };
    if let Some(built) = built {
        outcome
            .progress
            .insert_str(0, &format!("{}\n", built.progress.trim_end()));
        outcome.notices = built.notices;
        outcome.summary = built.summary;
        outcome.diagnostics = built.diagnostics;
    }
    finish(&analyzed, &outcome, cli)
}

/// The call-by-call view of a run (`tooling/40` §3.3): each call of the goal in source order with ✓ or ✗, then the goal
/// itself; on failure the values of the calls that ran follow their marks, cut as `runtime/30` R-RUN-20 says.
fn call_lines(run: &GoalRun) -> String {
    let failed = run.outcome.is_err();
    let width = run
        .calls
        .iter()
        .map(|call| call.callee.chars().count())
        .chain([run.goal.chars().count()])
        .max()
        .unwrap_or(0)
        + 2;
    let mut out = String::new();
    for call in &run.calls {
        let shown = match (call.status(), &call.run) {
            (CallStatus::Skipped, _) => "skipped".to_owned(),
            (CallStatus::Failed, _) => "✗".to_owned(),
            (CallStatus::Ok, Some(GoalRun { outcome: Ok(value), .. })) if failed => {
                format!("✓ {}", velme_ir::display_value(value))
            }
            (CallStatus::Ok, _) => "✓".to_owned(),
        };
        out.push_str(&format!("{:<width$}{shown}\n", call.callee));
    }
    let mark = if failed { "✗" } else { "✓" };
    out.push_str(&format!("{:<width$}{mark}\n", run.goal));
    out
}

/// `velme explain FILE --goal G`: the goal's calls as plain-language steps, from the call DAG alone (`tooling/40` §3.4,
/// `runtime/30` §9). It reads no lock, builds nothing and asks no provider.
fn explain_goal(cli: &Cli) -> u8 {
    let analyzed = analyze(cli);
    if let Err(bad) = settings(&analyzed) {
        return finish(&analyzed, &bad, cli);
    }
    let Some(program) = &analyzed.program else {
        return finish(&analyzed, &Outcome::file(&analyzed), cli);
    };
    let goal = cli.goal.as_deref().unwrap_or_default();
    let id = match find_goal(program, goal) {
        Ok(id) => id,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), cli),
    };
    let Some(text) = explain(program, id) else {
        return finish(&analyzed, &Outcome::with(vec![Diagnostic::internal_error()]), cli);
    };
    let outcome = Outcome {
        shown: text.clone(),
        diagnostics: Vec::new(),
        results: vec![GoalResult::new(goal, Ok(Some(Value::text(&text))))],
        ..Outcome::default()
    };
    finish(&analyzed, &outcome, cli)
}

/// `velme test FILE [--goal G]`: runs the `examples:` of every goal, or of `G`, leaf or composite, then its generated inputs,
/// with the locked artifacts of the goal and of those it calls and through the verifier `velme build` uses (`tooling/40` §2,
/// R-CLI-04, R-GOAL-22, D-107). With `--build` the file is built first; `--locked` only forbids that.
fn test(cli: &Cli) -> u8 {
    let analyzed = analyze(cli);
    let settings = match settings(&analyzed) {
        Ok(settings) => settings,
        Err(bad) => return finish(&analyzed, &bad, cli),
    };
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), cli);
    };
    let ids = match cli.goal.as_deref().map(|g| find_goal(program, g)).transpose() {
        Ok(Some(id)) => vec![id],
        Ok(None) => (0..program.goals.len()).map(GoalId).collect(),
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), cli),
    };
    let built = cli
        .build
        .then(|| build_phase(cli, &analyzed, (program, text, project), &settings));
    if let Some(built) = built.as_ref().filter(|b| build_stopped(b)) {
        return finish(&analyzed, built, cli);
    }
    let options = cli
        .jobs
        .map_or_else(Options::default, |jobs| Options::default().with_jobs(jobs));
    let mut outcome = Outcome::with(Vec::new());
    for id in ids {
        let Some(g) = program.goals.get(id.0) else { continue };
        let tested =
            registry(project, program, id).and_then(|registry| test_goal(program, id, text, &registry, &options));
        let line = match &tested {
            Ok(t) => format!("✓ {}", counted(t.examples, t.generated_inputs)),
            Err(_) => "✗".to_owned(),
        };
        outcome.progress.push_str(&format!("{}  {line}\n", g.name));
        outcome.results.push(GoalResult::new(&g.name, tested.map(|_| None)));
    }
    if let Some(built) = built {
        outcome
            .progress
            .insert_str(0, &format!("{}\n", built.progress.trim_end()));
        outcome.notices = built.notices;
        outcome.summary = built.summary;
        outcome.diagnostics = built.diagnostics;
    }
    finish(&analyzed, &outcome, cli)
}

/// "2 examples, 14 generated inputs" for what a passing `velme test` ran.
fn counted(examples: usize, generated: usize) -> String {
    let plural = |n: usize, noun: &str| format!("{n} {noun}{}", if n == 1 { "" } else { "s" });
    format!(
        "{}, {}",
        plural(examples, "example"),
        plural(generated, "generated input")
    )
}

/// `velme artifact FILE --goal G`: the goal's locked artifact, loaded with the checks and the codes `velme run` has
/// (`runtime/32` R-ART-10, R-ART-16), then its hash, its manifest and its IR (`tooling/40` R-CLI-22, D-107). It never
/// builds and never shows a stale artifact.
fn artifact(cli: &Cli) -> u8 {
    let analyzed = analyze(cli);
    if let Err(bad) = settings(&analyzed) {
        return finish(&analyzed, &bad, cli);
    }
    let (Some(program), Some(project)) = (&analyzed.program, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), cli);
    };
    let goal = cli.goal.as_deref().unwrap_or_default();
    let id = match find_goal(program, goal) {
        Ok(id) => id,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), cli),
    };
    let mut outcome = Outcome::with(Vec::new());
    let result = match locked_goal(project, program, id) {
        Ok(locked) => {
            let Some(text) = shown_artifact(&locked) else {
                return finish(&analyzed, &Outcome::with(vec![Diagnostic::internal_error()]), cli);
            };
            outcome.shown = text;
            outcome.escaped = true;
            let mut result = GoalResult::new(goal, Ok(None));
            result.artifact = Some(locked);
            result
        }
        Err(diagnostics) => GoalResult::new(goal, Err(diagnostics)),
    };
    outcome.results.push(result);
    finish(&analyzed, &outcome, cli)
}

/// The outcome of a command that takes no file (`velme gc`, `velme cache clean`): nothing of a source file to show.
fn no_file() -> Analyzed {
    Analyzed {
        path: String::new(),
        project: None,
        text: None,
        program: None,
        diagnostics: Vec::new(),
        settings: Ok(Settings::default()),
    }
}

/// `velme gc`: deletes the artifact files the project's lock doesn't name, and the temporary files left in `.velme/tmp`, and
/// prints how many (`tooling/40` R-CLI-23, D-108). The project is the one whose `velme.toml` is nearest above the current
/// directory, else the current directory.
fn gc(cli: &Cli) -> u8 {
    let analyzed = no_file();
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = cwd
        .ancestors()
        .find(|dir| dir.join(project::CONFIG_FILE).is_file())
        .unwrap_or(&cwd);
    let removed = Lock::require(root).map_err(|e| e.diagnostic()).and_then(|lock| {
        let keep: Vec<_> = lock.entries().iter().map(|e| e.artifact).collect();
        Store::new(root)
            .collect(&keep, std::time::SystemTime::now(), velme_runtime::GC_MIN_AGE)
            .map_err(|e| SourceFile::unreadable(velme_runtime::VELME_DIR, &e))
    });
    let mut outcome = Outcome::default();
    match removed {
        Ok(n) => {
            let s = if n == 1 { "" } else { "s" };
            outcome.shown = format!("Removed {n} unused file{s}.\n");
            outcome.summary = Some(serde_json::json!({ "removed": n }));
        }
        Err(diagnostic) => outcome.diagnostics.push(diagnostic),
    }
    finish(&analyzed, &outcome, cli)
}

/// `velme cache clean`: deletes the user-level WASM module cache if it exists, and is done if it doesn't (`tooling/40`
/// R-CLI-24, D-108). A cache directory that is a link is left alone.
fn cache_clean(cli: &Cli) -> u8 {
    let analyzed = no_file();
    let get = |name: &str| std::env::var_os(name);
    let mut outcome = Outcome::default();
    let cleaned = match config::wasm_cache_path(&get, config::Platform::current()) {
        None => Ok(false),
        Some(dir) => match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(SourceFile::unreadable(&display_path(&dir.to_string_lossy()), &e)),
            Ok(meta) if meta.file_type().is_symlink() => Err(SourceFile::unreadable(
                &display_path(&dir.to_string_lossy()),
                &std::io::Error::other("it is a symbolic link, which Velme doesn't follow"),
            )),
            Ok(_) => std::fs::remove_dir_all(&dir)
                .map(|()| true)
                .map_err(|e| SourceFile::unreadable(&display_path(&dir.to_string_lossy()), &e)),
        },
    };
    match cleaned {
        Ok(removed) => {
            outcome.shown = if removed {
                "Removed the WASM module cache.\n".to_owned()
            } else {
                "There is no WASM module cache to remove.\n".to_owned()
            };
            outcome.summary = Some(serde_json::json!({ "removed": u8::from(removed) }));
        }
        Err(diagnostic) => outcome.diagnostics.push(diagnostic),
    }
    finish(&analyzed, &outcome, cli)
}

/// The compact JSON of `value`.
fn compact<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_string(value).ok()
}

/// What `velme artifact` prints: the hash, the manifest's fields one `name: value` per line, a blank line, then the IR
/// as pretty JSON. The fields are named here, in the manifest's order, because a JSON map would sort them; a test keeps
/// the list whole.
fn shown_artifact(locked: &LockedGoal) -> Option<String> {
    let m = &locked.manifest;
    let mut lines = vec![
        ("format", ARTIFACT_FORMAT.to_owned()),
        ("goal", m.goal.clone()),
        ("kind", compact(&m.kind)?.trim_matches('"').to_owned()),
        ("signature", m.signature.to_string()),
        ("contract_key", m.contract_key.to_string()),
        ("synthesis_key", m.synthesis_key.to_string()),
        ("language_version", m.language_version.clone()),
        ("compiler_version", m.compiler_version.clone()),
        ("ir_version", m.ir_version.clone()),
        ("builtins_version", m.builtins_version.clone()),
    ];
    lines.extend(m.prompt_version.iter().map(|v| ("prompt_version", v.clone())));
    lines.push(("provider", m.provider.clone()));
    lines.extend(m.backend.iter().map(|v| ("backend", v.clone())));
    lines.extend(m.model_version.iter().map(|v| ("model_version", v.clone())));
    lines.push(("children", compact(&m.children)?));
    lines.push(("verification", compact(&m.verification)?));
    let mut out = format!("{}\n", locked.artifact);
    for (name, value) in lines {
        out.push_str(&format!("{name}: {value}\n"));
    }
    // The manifest's lines are escaped like any text; the IR is JSON, so it is escaped as JSON and stays parseable.
    let ir = serde_json::to_string_pretty(locked.ir.goal()).ok()?;
    Some(format!("{}\n{}\n", render::escape(&out), render::escape_json(&ir)))
}

/// The locked artifact of goal `id` of the file of `project` (R-ART-10, R-ART-16): never synthesized here.
fn locked_goal(project: &Project, program: &Program, id: GoalId) -> Result<LockedGoal, Vec<Diagnostic>> {
    let goal = program
        .goals
        .get(id.0)
        .ok_or_else(|| vec![Diagnostic::internal_error()])?;
    let lock = Lock::read(&project.root)
        .map_err(|e| vec![e.diagnostic()])?
        .unwrap_or_else(|| Lock::new(program.language_version.clone()));
    load(program, id, &project.file, &lock, &Store::new(&project.root))
        .map_err(|e| vec![e.diagnostic(&goal.name, goal.span)])
}

/// The locked artifacts of goal `id` and of every goal it calls (R-ART-10, R-ART-16): never synthesized here.
fn registry(project: &Project, program: &Program, id: GoalId) -> Result<Registry, Vec<Diagnostic>> {
    let lock = Lock::read(&project.root)
        .map_err(|e| vec![e.diagnostic()])?
        .unwrap_or_else(|| Lock::new(program.language_version.clone()));
    Registry::load(program, id, &project.file, &lock, &Store::new(&project.root))
}

/// The text of `--input`: a file, or standard input for `-` (`tooling/40` §3.1), read no further than the input limit.
fn read_input(input: &str) -> Result<String, Diagnostic> {
    if input == STDIN {
        return velme_runtime::read_input(std::io::stdin().lock(), STDIN_NAME);
    }
    let path = display_path(input);
    let file = std::fs::File::open(input).map_err(|e| SourceFile::unreadable(&path, &e))?;
    velme_runtime::read_input(file, &path)
}

/// A value as pretty JSON, by the one mapping (`tooling/40` §3.2, D-23): records' fields in declaration order.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(&OrderedValue(value)).unwrap_or_default()
}

impl Outcome {
    /// The outcome of analysis alone: its progress lines.
    fn file(analyzed: &Analyzed) -> Self {
        Outcome {
            progress: progress_lines(&analyzed.diagnostics),
            ..Outcome::default()
        }
    }

    /// An outcome that is only file diagnostics.
    fn with(diagnostics: Vec<Diagnostic>) -> Self {
        Outcome {
            diagnostics,
            ..Outcome::default()
        }
    }
}

/// Whether each of stdout and stderr is coloured (`tooling/40` R-CLI-28): `always` and `never` apply to both, and `auto` to a
/// stream by its own state, so stdout is coloured when it is a terminal and stderr when it is one, and `NO_COLOR`, set to
/// anything, turns both off.
fn stream_colors(color: Color, stdout_tty: bool, stderr_tty: bool, no_color: bool) -> (bool, bool) {
    match color {
        Color::Always => (true, true),
        Color::Never => (false, false),
        Color::Auto => (stdout_tty && !no_color, stderr_tty && !no_color),
    }
}

/// Whether `NO_COLOR` is set to any value (R-CLI-28).
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// The colours of this process's streams for `--color`.
fn colors(color: Color) -> (bool, bool) {
    stream_colors(
        color,
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        no_color(),
    )
}

/// Marks in already escaped stdout text painted: `✓` green and `✗` red. The text holds no escape byte, so what is painted
/// is only Velme's own marks (R-CLI-17).
fn paint(text: &str) -> String {
    text.replace('✓', "\u{1b}[32m✓\u{1b}[0m")
        .replace('✗', "\u{1b}[31m✗\u{1b}[0m")
}

/// Prints `outcome` and returns the exit code of every diagnostic in it (R-CLI-16).
fn finish(analyzed: &Analyzed, outcome: &Outcome, cli: &Cli) -> u8 {
    let path = &analyzed.path;
    let file: Vec<&Diagnostic> = analyzed.diagnostics.iter().chain(&outcome.diagnostics).collect();
    let all: Vec<Diagnostic> = file
        .iter()
        .copied()
        .chain(outcome.results.iter().flat_map(|r| &r.diagnostics))
        .cloned()
        .collect();
    let exit = exit_code(&all);
    if cli.json {
        let lines = LineIndex::new(analyzed.text.as_deref().unwrap_or(""));
        let results: Vec<JsonResult> = outcome
            .results
            .iter()
            .map(|r| JsonResult {
                goal: r.goal.clone(),
                status: r.status,
                diagnostics: r
                    .diagnostics
                    .iter()
                    .map(|d| JsonDiagnostic::new(d, path, &lines))
                    .collect(),
                result: r.result.as_ref().map(OrderedValue),
                artifact: r.artifact.as_ref().map(|a| ArtifactJson {
                    artifact: a.artifact.to_string(),
                    manifest: &a.manifest,
                    ir: a.ir.goal(),
                }),
                trace: r.trace.as_ref().map(|(run, file)| run.trace(file)),
                calls: r.calls.clone(),
            })
            .collect();
        let failed = exit != EXIT_OK || results.iter().any(|r| r.status == "failed");
        let envelope = Envelope {
            format: JSON_FORMAT,
            status: if failed { "failed" } else { "ok" },
            results,
            diagnostics: file.iter().map(|d| JsonDiagnostic::new(d, path, &lines)).collect(),
            notices: outcome.notices.clone(),
            summary: outcome.summary.clone(),
        };
        match serde_json::to_string_pretty(&envelope) {
            Ok(out) => print_out(&format!("{}\n", render::escape_json(&out))),
            Err(_) => return EXIT_INTERNAL,
        }
    } else {
        let (out_color, err_color) = colors(cli.color);
        let show = |text: &str| {
            let text = render::escape(text);
            if out_color { paint(&text) } else { text }
        };
        // Progress first, so the lines for what passed read above the errors (`tooling/40` §3.3); `-q` drops them.
        if !cli.quiet {
            print_out(&show(&outcome.progress));
        }
        if outcome.escaped {
            print_out(&outcome.shown);
        } else {
            print_out(&show(&outcome.shown));
        }
        let text = analyzed.text.as_deref();
        let mut err = render::render_human(&analyzed.diagnostics, path, text, err_color);
        // What a command adds without a place in the file, such as an unknown `--goal` or an unreadable lock, is shown
        // without source lines.
        let (placed, unplaced): (Vec<Diagnostic>, Vec<Diagnostic>) = all
            .into_iter()
            .skip(analyzed.diagnostics.len())
            .partition(|d| d.span != Span::default());
        err.push_str(&render::render_human(&unplaced, path, None, err_color));
        err.push_str(&render::render_human(&placed, path, text, err_color));
        if !err.is_empty() {
            print_err(&err);
        }
    }
    exit
}

/// The [`CHECK_LINES`] up to the first whose phase didn't pass: one that has an error from its own or an earlier
/// phase, or from outside the checked phases (an unreadable file).
fn progress_lines(diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    for (i, (line, _)) in CHECK_LINES.iter().enumerate() {
        let later: Vec<&str> = CHECK_LINES
            .iter()
            .skip(i + 1)
            .flat_map(|(_, p)| p.iter().copied())
            .collect();
        let failed = diagnostics
            .iter()
            .any(|d| d.is_error() && !later.iter().any(|p| d.code.as_str().starts_with(p)));
        if failed {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The same failure again, for a second report of it.
fn clone_error(error: &std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), error.to_string())
}

/// Runs `work` on a thread with [`STACK`]; `None` only if the thread couldn't start or `work` panicked, which is a bug
/// (R-SYN-19).
fn on_big_stack<T: Send>(work: impl FnOnce() -> T + Send) -> Option<T> {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn_scoped(scope, work)
            .ok()?;
        worker.join().ok()
    })
}

/// The `--json` envelope (`tooling/40` §3.2). `diagnostics` holds the ones that belong to the file rather than to one
/// goal, such as syntax errors (D-72).
#[derive(Serialize)]
struct Envelope<'a> {
    format: &'static str,
    status: &'static str,
    results: Vec<JsonResult<'a>>,
    diagnostics: Vec<JsonDiagnostic>,
    notices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<serde_json::Value>,
}

/// One goal in the `--json` envelope (R-CLI-15).
#[derive(Serialize)]
struct JsonResult<'a> {
    goal: String,
    status: &'static str,
    diagnostics: Vec<JsonDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<OrderedValue<'a>>,
    /// What `velme artifact` shows, beside `result`, which is only ever a goal's value (D-111).
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<ArtifactJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<Trace<'a>>,
    /// The goals it called, in source order (R-CLI-27).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    calls: Vec<CallJson>,
}

/// One call of a goal in the `--json` envelope (R-CLI-27): its binding, the goal called and how it ended.
#[derive(Serialize, Clone)]
struct CallJson {
    binding: String,
    goal: String,
    status: &'static str,
}

impl CallJson {
    fn of(call: &velme_runtime::CallRun) -> CallJson {
        CallJson {
            binding: call.binding.clone(),
            goal: call.callee.clone(),
            status: match call.status() {
                CallStatus::Ok => "ok",
                CallStatus::Failed => "failed",
                CallStatus::Skipped => "skipped",
            },
        }
    }
}

/// The `artifact` of `velme artifact --json` (R-CLI-22, D-111): the artifact's hash, its manifest and its IR.
#[derive(Serialize)]
struct ArtifactJson<'a> {
    artifact: String,
    manifest: &'a velme_runtime::Manifest,
    ir: &'a velme_ir::Goal,
}

/// Paths are shown with `/` separators on every platform (R-CLI-19).
fn display_path(arg: &str) -> String {
    if cfg!(windows) {
        arg.replace('\\', "/")
    } else {
        arg.to_owned()
    }
}

/// The process exit code for a set of diagnostics: warnings never fail (D-69), and errors follow R-CLI-16.
fn exit_code(diagnostics: &[Diagnostic]) -> u8 {
    diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| code_exit(d.code))
        .min_by_key(|exit| EXIT_PRECEDENCE.iter().position(|e| e == exit))
        .unwrap_or(EXIT_OK)
}

/// The `tooling/40` §4 row for each code.
fn code_exit(code: Code) -> u8 {
    use Code::*;
    match code {
        UnexpectedToken
        | InconsistentIndentation
        | TabIndentation
        | ReservedWord
        | UnterminatedText
        | UnsupportedLanguageVersion
        | LintWarning
        | UnknownType
        | UnknownName
        | DuplicateDeclaration
        | TypeMismatch
        | UnknownField
        | InvalidOperandType
        | NullableAccess
        | RecursiveType
        | UnknownGoal
        | CallArityMismatch
        | InvalidCall
        | CallCycle
        | BindingUsedBeforeDefinition
        | DuplicateBinding
        | GoalHasNoBody
        | InvalidBudget => EXIT_INVALID_PROGRAM,
        IRSchemaInvalid
        | IRInvalid
        | SynthesisFailed
        | ProviderUnavailable
        | ProviderNotConfigured
        | BackendFailed
        | PlanUnclear
        | SynthesisPending
        | SynthesisBlocked
        | VerificationFailed => EXIT_BUILD_FAILED,
        CheckFailed | ExampleFailed | BudgetExceeded | ArithmeticError | Timeout | MemoryLimitExceeded
        | CallLimitExceeded | SizeLimitExceeded | CapabilityDenied => EXIT_EXECUTION_FAILED,
        InternalError => EXIT_INTERNAL,
        ArtifactUnavailable | LockStale | ArtifactCorrupt => EXIT_ARTIFACT,
        FileError | InvalidInput | GoalNotFound => EXIT_USAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_is_snapshotted() {
        insta::assert_snapshot!(version_line(), @"velme 0.1.0");
    }

    #[test]
    fn every_code_has_an_exit_row() {
        for &code in Code::ALL {
            let exit = code_exit(code);
            assert!(EXIT_PRECEDENCE.contains(&exit), "{code:?} maps to {exit}");
        }
    }

    /// A static call limit fails the call graph line, not the ones before it (AC-RUN-06).
    #[test]
    fn call_limit_is_reported_with_the_call_graph() {
        let diag = |code| Diagnostic::new(code, Default::default(), "");
        assert_eq!(
            progress_lines(&[diag(Code::CallLimitExceeded)]),
            "✓ Parsed\n✓ Types valid\n"
        );
        assert_eq!(
            progress_lines(&[diag(Code::InvalidBudget)]),
            "✓ Parsed\n✓ Types valid\n"
        );
    }

    /// `auto` decides for each stream by its own state; `NO_COLOR` turns both off, and `always` and `never` apply to both
    /// (AC-CLI-25, R-CLI-28).
    #[test]
    fn ac_cli_25_auto_colours_each_stream_by_its_own_state() {
        // (stdout is a terminal, stderr is a terminal, NO_COLOR set) -> (stdout coloured, stderr coloured)
        let auto = |out, err, no| stream_colors(Color::Auto, out, err, no);
        assert_eq!(auto(false, true, false), (false, true), "only stderr is a terminal");
        assert_eq!(auto(true, false, false), (true, false));
        assert_eq!(auto(true, true, false), (true, true));
        assert_eq!(auto(false, false, false), (false, false));
        assert_eq!(auto(true, true, true), (false, false), "NO_COLOR");
        assert_eq!(stream_colors(Color::Always, false, false, true), (true, true));
        assert_eq!(stream_colors(Color::Never, true, true, false), (false, false));
    }

    #[test]
    fn internal_error_exits_70() {
        assert_eq!(exit_code(&[Diagnostic::internal_error()]), EXIT_INTERNAL);
    }

    /// A bug that stops a `--json` command still ends in the envelope, failed with `VL0607` (R-CLI-15).
    #[test]
    fn a_stopped_json_command_prints_the_envelope() {
        let out = internal_envelope("game.velme").expect("an envelope");
        let envelope: serde_json::Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(envelope["format"], JSON_FORMAT);
        assert_eq!(envelope["status"], "failed");
        assert_eq!(envelope["results"], serde_json::json!([]));
        assert_eq!(envelope["diagnostics"][0]["code"], "VL0607");
        assert_eq!(envelope["diagnostics"][0]["file"], "game.velme");
    }

    #[test]
    fn exit_precedence_follows_r_cli_16() {
        let diag = |code| Diagnostic::new(code, Default::default(), "");
        let exit = |codes: &[Code]| exit_code(&codes.iter().map(|&c| diag(c)).collect::<Vec<_>>());
        assert_eq!(exit(&[]), EXIT_OK);
        assert_eq!(exit(&[Code::LintWarning]), EXIT_OK);
        assert_eq!(exit(&[Code::CheckFailed, Code::LockStale]), EXIT_ARTIFACT);
        assert_eq!(exit(&[Code::LockStale, Code::InvalidInput]), EXIT_USAGE);
        assert_eq!(exit(&[Code::InvalidInput, Code::InternalError]), EXIT_INTERNAL);
        assert_eq!(
            exit(&[Code::VerificationFailed, Code::UnexpectedToken]),
            EXIT_INVALID_PROGRAM
        );
    }
}
