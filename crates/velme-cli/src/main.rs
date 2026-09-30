//! `velme` binary: the only crate that prints, reads the environment and picks exit codes (R-CMP-03).
#![forbid(unsafe_code)]

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
    BuildInput, CallStatus, EntryError, GoalRun, Lock, LockedGoal, Options, OrderedValue, Registry, Source, Status,
    Store, Trace, decode_inputs, explain, find_goal, load, run_goal, test_leaf,
};
use velme_sema::hir::{GoalId, GoalKind, Program};
use velme_syntax::SourceFile;
use velme_synth::{Replay, SynthBackend, SynthOptions};

use crate::project::Project;
use crate::synth::{Chosen, model_name};

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
                     velme build FILE [--provider NAME] [--model ID] [--external-command CMD] [-v] [--json]\n       \
                     velme run FILE --goal G [--input FILE.json|-] [--arg NAME=JSON]... [--jobs N] [--json]\n       \
                     velme test FILE [--goal G] [--json]\n       \
                     velme explain FILE --goal G [--json]\n       \
                     velme trace FILE --goal G [--input FILE.json|-] [--arg NAME=JSON]... [--jobs N] [--json]\n       \
                     velme --version";

/// The `velme check` line for locked IR (`tooling/40` §3.3), shown when the project has a lock.
const IR_LINE: &str = "✓ IR valid        ";

/// `--input`'s name for standard input (`tooling/40` §3.1).
const STDIN: &str = "-";

/// How standard input is named when it can't be read.
const STDIN_NAME: &str = "standard input";

fn version_line() -> String {
    format!("velme {}", env!("CARGO_PKG_VERSION"))
}

enum Command {
    Version,
    Check {
        file: String,
        json: bool,
    },
    Build {
        file: String,
        json: bool,
        provider: Option<String>,
        model: Option<String>,
        external_command: Option<String>,
        verbose: bool,
    },
    Run {
        file: String,
        json: bool,
        goal: String,
        input: Option<String>,
        args: Vec<(String, String)>,
        jobs: Option<usize>,
    },
    Test {
        file: String,
        json: bool,
        goal: Option<String>,
    },
    Explain {
        file: String,
        json: bool,
        goal: String,
    },
    Trace {
        file: String,
        json: bool,
        goal: String,
        input: Option<String>,
        args: Vec<(String, String)>,
        jobs: Option<usize>,
    },
}

/// `--json` is a global flag (`tooling/40` §2.1), so it may come before or after the command; the other flags follow
/// the command, in any order.
fn parse_args(args: &[String]) -> Option<Command> {
    let json = args.iter().any(|a| a == "--json");
    let mut rest = args.iter().map(String::as_str).filter(|a| *a != "--json");
    let command = rest.next()?;
    if matches!(command, "--version" | "-V") {
        return (!json && rest.next().is_none()).then_some(Command::Version);
    }
    let (mut file, mut goal, mut input, mut pairs, mut jobs) = (None, None, None, Vec::new(), None);
    let (mut provider, mut model, mut external_command, mut verbose) = (None, None, None, false);
    while let Some(arg) = rest.next() {
        match arg {
            "--provider" if provider.is_none() => provider = Some(rest.next()?.to_owned()),
            "--model" if model.is_none() => model = Some(rest.next()?.to_owned()),
            "--external-command" if external_command.is_none() => external_command = Some(rest.next()?.to_owned()),
            "-v" | "--verbose" => verbose = true,
            "--goal" if goal.is_none() => goal = Some(rest.next()?.to_owned()),
            "--input" if input.is_none() => input = Some(rest.next()?.to_owned()),
            "--jobs" if jobs.is_none() => jobs = Some(rest.next()?.parse().ok().filter(|n| *n >= 1)?),
            "--arg" => {
                let (name, value) = rest.next()?.split_once('=')?;
                pairs.push((name.to_owned(), value.to_owned()));
            }
            _ if arg.starts_with('-') || file.is_some() => return None,
            _ => file = Some(arg.to_owned()),
        }
    }
    let file = file?;
    let inputs = input.is_some() || !pairs.is_empty();
    if command == "build" {
        let plain = goal.is_none() && !inputs && jobs.is_none();
        return plain.then_some(Command::Build {
            file,
            json,
            provider,
            model,
            external_command,
            verbose,
        });
    }
    if provider.is_some() || model.is_some() || external_command.is_some() || verbose {
        return None;
    }
    match command {
        "check" if goal.is_none() && !inputs && jobs.is_none() => Some(Command::Check { file, json }),
        "run" => Some(Command::Run {
            file,
            json,
            goal: goal?,
            input,
            args: pairs,
            jobs,
        }),
        "trace" => Some(Command::Trace {
            file,
            json,
            goal: goal?,
            input,
            args: pairs,
            jobs,
        }),
        "explain" if !inputs && jobs.is_none() => Some(Command::Explain {
            file,
            json,
            goal: goal?,
        }),
        "test" if !inputs && jobs.is_none() => Some(Command::Test { file, json, goal }),
        _ => None,
    }
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
    let file = match parse_args(args) {
        Some(
            Command::Check { file, json: true }
            | Command::Run { file, json: true, .. }
            | Command::Test { file, json: true, .. }
            | Command::Build { file, json: true, .. },
        ) => file,
        _ => {
            print_err(&format!("{}\n", Diagnostic::internal_error().message));
            return EXIT_INTERNAL;
        }
    };
    if let Some(out) = internal_envelope(&shown_path(&Project::of(Path::new(&file)), &file)) {
        print_out(&out);
    }
    EXIT_INTERNAL
}

/// The `--json` envelope of a command on the file shown as `path` that a bug stopped: `VL0607` alone.
fn internal_envelope(path: &str) -> Option<String> {
    let envelope = Envelope {
        format: JSON_FORMAT,
        status: "failed",
        results: Vec::new(),
        diagnostics: vec![JsonDiagnostic::new(
            &Diagnostic::internal_error(),
            path,
            &LineIndex::new(""),
        )],
        notices: Vec::new(),
        summary: None,
    };
    let out = serde_json::to_string_pretty(&envelope).ok()?;
    Some(format!("{}\n", render::escape_json(&out)))
}

/// Runs the command `args` name, and gives its exit code.
fn command(args: &[String]) -> u8 {
    match parse_args(args) {
        Some(Command::Version) => {
            print_out(&format!("{}\n", version_line()));
            EXIT_OK
        }
        Some(Command::Check { file, json }) => check(&file, json),
        Some(Command::Build {
            file,
            json,
            provider,
            model,
            external_command,
            verbose,
        }) => {
            let flags = BuildFlags {
                provider: provider.as_deref(),
                model: model.as_deref(),
                external_command: external_command.as_deref(),
                verbose,
            };
            build_command(&file, json, &flags)
        }
        Some(Command::Run {
            file,
            json,
            goal,
            input,
            args,
            jobs,
        }) => run(&file, json, &goal, input.as_deref(), &args, jobs, false),
        Some(Command::Trace {
            file,
            json,
            goal,
            input,
            args,
            jobs,
        }) => run(&file, json, &goal, input.as_deref(), &args, jobs, true),
        Some(Command::Explain { file, json, goal }) => explain_goal(&file, json, &goal),
        Some(Command::Test { file, json, goal }) => test(&file, json, goal.as_deref()),
        None => {
            print_err(&format!("{USAGE}\n"));
            EXIT_USAGE
        }
    }
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
}

/// How the file `arg` is shown: from its project's root (R-CLI-19), or as given if it can't be found.
fn shown_path(project: &std::io::Result<Project>, arg: &str) -> String {
    project.as_ref().map_or_else(|_| display_path(arg), |p| p.file.clone())
}

fn analyze(arg: &str) -> Analyzed {
    let project = Project::of(Path::new(arg));
    let path = shown_path(&project, arg);
    let read = std::fs::read(arg).and_then(|bytes| project.as_ref().map(|_| bytes).map_err(clone_error));
    let (text, program, diagnostics) = match read {
        Err(err) => (None, None, vec![SourceFile::unreadable(&path, &err)]),
        Ok(bytes) => match SourceFile::from_bytes(path.clone(), bytes.clone()) {
            // A thread of its own, so a bug in analysis is reported like any other (R-SYN-19).
            Ok(file) => match on_big_stack(|| velme_sema::analyze(&file)) {
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
    }
}

/// What a command found, to print as text or as the `--json` envelope (`tooling/40` §3.2).
#[derive(Default)]
struct Outcome {
    /// Printed to stdout in human mode, above the diagnostics.
    progress: String,
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
        }
    }
}

/// `velme check FILE`: phases 1–6 (`compiler/20` §3), then the locked IR of the file's goals if the project has a
/// lock (`tooling/40` §2). Check evaluation follows in a later phase.
fn check(arg: &str, json: bool) -> u8 {
    let analyzed = analyze(arg);
    let mut outcome = Outcome::file(&analyzed);
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
    }
    finish(&analyzed, &outcome, json)
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

/// The provider `name` names, or why there is none (`tooling/40` §2.1, R-CLI-12), recorded as replay fixtures when
/// `VELME_SYNTH_RECORD=1` says so (`compiler/22` R-SYNTH-43); the `replay` provider is never recorded onto itself.
fn backend(name: &str, project: &Project, flags: &BuildFlags) -> Result<Chosen, Diagnostic> {
    let (backend, notice) = provider(name, project, flags)?;
    if name != "replay" && std::env::var("VELME_SYNTH_RECORD").is_ok_and(|v| v == "1") {
        let recorder = velme_synth::Recorder::new(backend, project.root.join(REPLAY_DIR));
        return Ok((Box::new(recorder), notice));
    }
    Ok((backend, notice))
}

/// The provider `name` names, and the notice that says what it sends (`tooling/41` R-SEC-12): `replay` reads the
/// fixtures of the project's `tests/fixtures/synth`; `scripted` is there only in a build with the `test-provider`
/// feature (D-94).
fn provider(name: &str, project: &Project, flags: &BuildFlags) -> Result<Chosen, Diagnostic> {
    match name {
        "replay" => Ok((
            Box::new(Replay::new(project.root.join(REPLAY_DIR))),
            "Nothing is sent: the replay provider answers from recorded fixtures.".to_owned(),
        )),
        #[cfg(feature = "test-provider")]
        "scripted" => {
            let path = std::env::var("VELME_SYNTH_SCRIPT").map_err(|_| {
                Diagnostic::new(
                    Code::InvalidInput,
                    Span::default(),
                    "The scripted provider needs `VELME_SYNTH_SCRIPT`, the path of its script.",
                )
            })?;
            let text = std::fs::read_to_string(&path).map_err(|e| SourceFile::unreadable(&display_path(&path), &e))?;
            let scripted = velme_synth::Scripted::from_script(&text)
                .map_err(|e| Diagnostic::new(Code::InvalidInput, Span::default(), e.to_string()))?;
            Ok((
                Box::new(scripted),
                "Nothing is sent: the scripted provider answers from its script.".to_owned(),
            ))
        }
        "anthropic" => anthropic(flags.model),
        "ollama" => synth::ollama(flags.model),
        "external" => synth::external(flags.external_command, project),
        _ => Err(Diagnostic::new(
            Code::InvalidInput,
            Span::default(),
            format!("I don't know a provider called `{name}`."),
        )
        .with_help("the providers are anthropic, ollama, external and replay")),
    }
}

/// The `anthropic` provider, if the key and the model it needs are set (`tooling/40` §5.2, R-CLI-12): the model comes
/// from `--model`, else `VELME_MODEL`; the key only from the environment (`tooling/41` R-SEC-05), and it is read again
/// at each request, never held here.
fn anthropic(flag: Option<&str>) -> Result<(Box<dyn SynthBackend>, String), Diagnostic> {
    let not_configured = |message: &str, help: &str| {
        Diagnostic::new(Code::ProviderNotConfigured, Span::default(), message).with_help(help)
    };
    if velme_synth::ApiKey::from_env().is_none() {
        return Err(not_configured(
            "The Anthropic provider needs an API key, and there isn't one.",
            "set `VELME_API_KEY` (or `ANTHROPIC_API_KEY`) in the environment; Velme reads a key from nowhere else",
        ));
    }
    let model = flag
        .map(str::to_owned)
        .or_else(|| std::env::var("VELME_MODEL").ok())
        .filter(|m| !m.trim().is_empty());
    let Some(model) = model else {
        return Err(not_configured(
            "The Anthropic provider needs a model, and there isn't one.",
            "pass `--model ID`, or set `VELME_MODEL`, to the model id to use",
        ));
    };
    Ok((
        Box::new(velme_synth::Anthropic::new(velme_synth::AnthropicConfig::new(model))),
        "Sending your plans, types, checks and examples to Anthropic to write the code.".to_owned(),
    ))
}

/// The provider that can't be used: every goal that needs it is `VL0405` (`tooling/40` R-CLI-12).
struct NotConfigured;

#[async_trait::async_trait]
impl SynthBackend for NotConfigured {
    async fn identify(&self) -> Result<velme_synth::Identity, velme_synth::ProviderError> {
        Err(velme_synth::ProviderError::NotConfigured)
    }

    fn open(
        &self,
        _identity: &velme_synth::Identity,
    ) -> Result<Box<dyn velme_synth::SynthProvider>, velme_synth::ProviderError> {
        Err(velme_synth::ProviderError::NotConfigured)
    }
}

/// Where a project's replay fixtures are unless `[synthesis] replay_dir` says otherwise (`tooling/40` §5.1).
const REPLAY_DIR: &str = "tests/fixtures/synth";

/// The `[synthesis]` settings of a build with provider `name`: `external` defaults to no retries, since a deterministic
/// backend gives the same answer again (`compiler/22` R-SYNTH-30). A replay follows the provider it replays, so a replayed
/// external build asks for what the recorded one did.
fn synth_options(name: &str, project: &Project) -> SynthOptions {
    let recorded = || {
        let text = std::fs::read_to_string(project.root.join(REPLAY_DIR).join(velme_synth::IDENTITY_FILE)).ok()?;
        velme_ir::from_json_str::<velme_synth::ReplayIdentity>(&text).ok()
    };
    let external = name == "external" || (name == "replay" && recorded().is_some_and(|r| r.provider == "external"));
    SynthOptions {
        max_retries: if external {
            0
        } else {
            SynthOptions::default().max_retries
        },
        ..SynthOptions::default()
    }
}

/// The flags of `velme build` that choose and set up the provider (`tooling/40` §2.1).
struct BuildFlags<'a> {
    provider: Option<&'a str>,
    model: Option<&'a str>,
    external_command: Option<&'a str>,
    verbose: bool,
}

/// `velme build FILE`: checks the file, then gives every goal without a fresh lock entry a verified artifact and a lock
/// entry (`tooling/40` §2, `runtime/32` R-ART-15). `provider` is `--provider`; with `verbose`, every attempt of a
/// failed goal is listed (`compiler/22` R-SYNTH-13).
fn build_command(arg: &str, json: bool, flags: &BuildFlags) -> u8 {
    let analyzed = analyze(arg);
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), json);
    };
    let name = flags.provider.unwrap_or(DEFAULT_PROVIDER);
    let verbose = flags.verbose;
    let chosen = backend(name, project, flags);
    // A provider that can't be used doesn't stop a build that needs none; a name or script that is wrong does.
    if let Err(diagnostic) = &chosen
        && diagnostic.code == Code::InvalidInput
    {
        return finish(&analyzed, &Outcome::with(vec![diagnostic.clone()]), json);
    }
    let mut notices = Vec::new();
    let mut announce = |line: &str| {
        if json && !line.is_empty() {
            notices.push(line.to_owned());
        } else if !line.is_empty() {
            print_err(&format!("{line}\n"));
        }
    };
    let (backend, notice): (&dyn SynthBackend, &str) = match &chosen {
        Ok((backend, notice)) => (backend.as_ref(), notice.as_str()),
        Err(_) => (&NotConfigured, ""),
    };
    let input = BuildInput {
        program,
        source: text,
        project: &project.root,
        file: &project.file,
        backend: Some(backend),
        options: synth_options(name, project),
        run: Options::default(),
    };
    let report = velme_runtime::build(&input, &mut || announce(notice));
    let mut outcome = Outcome::file(&analyzed);
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
        if let Err(why) = &chosen {
            for d in diagnostics.iter_mut().filter(|d| d.code == Code::ProviderNotConfigured) {
                d.message.clone_from(&why.message);
                d.help.clone_from(&why.help);
            }
        } else if name == "ollama" {
            // The server doesn't have the model (`compiler/22` R-SYNTH-24).
            let model = model_name(flags.model);
            for d in diagnostics.iter_mut().filter(|d| d.code == Code::ProviderNotConfigured) {
                d.message = format!("The Ollama server doesn't have the model `{model}`.");
                d.help = Some(format!(
                    "run `ollama pull {model}`, or choose another model with `--model`"
                ));
            }
        }
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
        outcome.results.push(GoalResult {
            goal: goal.goal.clone(),
            status,
            diagnostics,
            result: None,
            trace: None,
        });
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
    finish(&analyzed, &outcome, json)
}

/// The provider of a build that doesn't name one (`tooling/40` §2.1).
const DEFAULT_PROVIDER: &str = "anthropic";

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
/// interpreter, its calls wave by wave, then its checks (`tooling/40` §2, `runtime/30` §4). `jobs` is `--jobs`. With
/// `traced` (`velme trace`) the whole execution trace is printed too (`runtime/30` §8).
fn run(
    arg: &str,
    json: bool,
    goal: &str,
    input: Option<&str>,
    args: &[(String, String)],
    jobs: Option<usize>,
    traced: bool,
) -> u8 {
    let analyzed = analyze(arg);
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), json);
    };
    let id = match find_goal(program, goal) {
        Ok(id) => id,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), json),
    };
    let input = match input.map(read_input).transpose() {
        Ok(input) => input,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), json),
    };
    // Input and lock problems are both reported, so the exit code follows R-CLI-16's precedence.
    let inputs = decode_inputs(program, id, input.as_deref(), args);
    let registry = registry(project, program, id);
    let options = jobs.map_or_else(Options::default, |jobs| Options::default().with_jobs(jobs));
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
        Some(executed) if traced => executed.trace_text(),
        Some(executed) => call_lines(executed),
        None => format!("{goal}  ✗\n"),
    };
    if let Ok(Some(value)) = &result {
        progress.push_str(&format!("\nResult:\n{}\n", pretty(value)));
    }
    let mut goal_result = GoalResult::new(goal, result);
    if traced {
        goal_result.trace = executed.map(|executed| (executed, analyzed.path.clone()));
    }
    let outcome = Outcome {
        progress,
        diagnostics: Vec::new(),
        results: vec![goal_result],
        ..Outcome::default()
    };
    finish(&analyzed, &outcome, json)
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
fn explain_goal(arg: &str, json: bool, goal: &str) -> u8 {
    let analyzed = analyze(arg);
    let Some(program) = &analyzed.program else {
        return finish(&analyzed, &Outcome::file(&analyzed), json);
    };
    let id = match find_goal(program, goal) {
        Ok(id) => id,
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), json),
    };
    let Some(text) = explain(program, id) else {
        return finish(&analyzed, &Outcome::with(vec![Diagnostic::internal_error()]), json);
    };
    let outcome = Outcome {
        progress: text.clone(),
        diagnostics: Vec::new(),
        results: vec![GoalResult::new(goal, Ok(Some(Value::text(&text))))],
        ..Outcome::default()
    };
    finish(&analyzed, &outcome, json)
}

/// `velme test FILE [--goal G]`: runs the `examples:` of each leaf goal, or of `G`, with its locked artifact, and its
/// checks on each (`tooling/40` §2, R-GOAL-22). Generated inputs follow with the test-input generator; goals with calls
/// are skipped: testing composite goals follows in a later phase.
fn test(arg: &str, json: bool, goal: Option<&str>) -> u8 {
    let analyzed = analyze(arg);
    let (Some(program), Some(text), Some(project)) = (&analyzed.program, &analyzed.text, &analyzed.project) else {
        return finish(&analyzed, &Outcome::file(&analyzed), json);
    };
    let ids = match goal.map(|g| find_goal(program, g)).transpose() {
        Ok(Some(id)) => vec![id],
        Ok(None) => (0..program.goals.len()).map(GoalId).collect(),
        Err(diag) => return finish(&analyzed, &Outcome::with(vec![diag]), json),
    };
    let mut outcome = Outcome::with(Vec::new());
    for id in ids {
        let Some(g) = program.goals.get(id.0) else { continue };
        if leaf(program, id).is_none() {
            outcome
                .progress
                .push_str(&format!("{}  skipped: it calls other goals\n", g.name));
            outcome.results.push(GoalResult {
                goal: g.name.clone(),
                status: "skipped",
                diagnostics: Vec::new(),
                result: None,
                trace: None,
            });
            continue;
        }
        let tested = locked_goal(project, program, id)
            .and_then(|locked| test_leaf(program, id, text, &locked, &Options::default()));
        let line = match &tested {
            Ok(1) => "✓ 1 example".to_owned(),
            Ok(n) => format!("✓ {n} examples"),
            Err(_) => "✗".to_owned(),
        };
        outcome.progress.push_str(&format!("{}  {line}\n", g.name));
        outcome.results.push(GoalResult::new(&g.name, tested.map(|_| None)));
    }
    finish(&analyzed, &outcome, json)
}

/// The name of goal `id` if it is a leaf goal, the only kind `velme test` runs yet.
fn leaf(program: &Program, id: GoalId) -> Option<&str> {
    program
        .goals
        .get(id.0)
        .filter(|g| g.kind == GoalKind::Leaf)
        .map(|g| g.name.as_str())
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
            diagnostics: Vec::new(),
            results: Vec::new(),
            ..Outcome::default()
        }
    }

    /// An outcome that is only file diagnostics.
    fn with(diagnostics: Vec<Diagnostic>) -> Self {
        Outcome {
            progress: String::new(),
            diagnostics,
            results: Vec::new(),
            ..Outcome::default()
        }
    }
}

/// Prints `outcome` and returns the exit code of every diagnostic in it (R-CLI-16).
fn finish(analyzed: &Analyzed, outcome: &Outcome, json: bool) -> u8 {
    let path = &analyzed.path;
    let file: Vec<&Diagnostic> = analyzed.diagnostics.iter().chain(&outcome.diagnostics).collect();
    let all: Vec<Diagnostic> = file
        .iter()
        .copied()
        .chain(outcome.results.iter().flat_map(|r| &r.diagnostics))
        .cloned()
        .collect();
    let exit = exit_code(&all);
    if json {
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
                trace: r.trace.as_ref().map(|(run, file)| run.trace(file)),
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
        // Progress first, so the lines for what passed read above the errors (`tooling/40` §3.3).
        print_out(&render::escape(&outcome.progress));
        let text = analyzed.text.as_deref();
        let mut err = render::render_human(&analyzed.diagnostics, path, text, use_color());
        // What a command adds without a place in the file, such as an unknown `--goal` or an unreadable lock, is shown
        // without source lines.
        let (placed, unplaced): (Vec<Diagnostic>, Vec<Diagnostic>) = all
            .into_iter()
            .skip(analyzed.diagnostics.len())
            .partition(|d| d.span != Span::default());
        err.push_str(&render::render_human(&unplaced, path, None, use_color()));
        err.push_str(&render::render_human(&placed, path, text, use_color()));
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
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<Trace<'a>>,
}

/// Paths are shown with `/` separators on every platform (R-CLI-19).
fn display_path(arg: &str) -> String {
    if cfg!(windows) {
        arg.replace('\\', "/")
    } else {
        arg.to_owned()
    }
}

/// Colour only on a terminal, and never when `NO_COLOR` is set (R-CMP-15).
fn use_color() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
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
