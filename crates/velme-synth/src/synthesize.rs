//! The retry loop (`compiler/22` §5): ask the provider for a candidate, validate it, verify it, and on failure ask again
//! with what Velme found, up to `max_retries` times. One goal at a time (R-SYNTH-01, D-93).

use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::{Fingerprint, Origin, Request, ValidIr, calls, from_json_str, validate_detailed};
use velme_sema::hir::{GoalId, Program};

use crate::attempt::{Rejection, clean_line, clean_text, summary};
use crate::compact;
use crate::options::{ReplyFormat, RetryHistory, SynthOptions};
use crate::provider::{ProviderError, SynthLimits, SynthProvider, Usage};
use crate::request::{AttemptFeedback, SynthRequest, build_request};
use crate::verify::{ChildRunner, Verdict, Verified, verify};

/// What one build's synthesis has done so far: the calls made and the tokens used, and the error that put the provider
/// out of reach, after which it is contacted no more (R-SYNTH-45, D-93).
#[derive(Debug, Clone)]
pub struct Session {
    options: SynthOptions,
    calls: usize,
    stopped: Option<ProviderError>,
    usage: Usage,
}

impl Session {
    /// A session for one build.
    pub fn new(options: SynthOptions) -> Self {
        Session {
            options,
            calls: 0,
            stopped: None,
            usage: Usage::default(),
        }
    }

    /// The provider calls made: one per `complete()` (R-SYNTH-21).
    pub fn calls(&self) -> usize {
        self.calls
    }

    /// The tokens used in all.
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// The error that ended a goal with `VL0404` or `VL0405`, so no goal reaches the provider again (R-SYNTH-45).
    pub fn stopped(&self) -> Option<&ProviderError> {
        self.stopped.as_ref()
    }

    /// Stops all contact with the provider, after `error` showed it can't be used: the identity step or a call failed
    /// in a way [`reaches_no_further`].
    pub fn stop(&mut self, error: &ProviderError) {
        self.stopped.get_or_insert_with(|| error.clone());
    }
}

/// One goal to synthesize.
pub struct Task<'a> {
    /// The checked program.
    pub program: &'a Program,
    /// The goal, a leaf or composite one.
    pub goal: GoalId,
    /// The source text of the program.
    pub source: &'a str,
    /// The goal's `contract_key`.
    pub contract_key: Fingerprint,
    /// The goal's `synthesis_key`, which names its replay fixture.
    pub synthesis_key: Fingerprint,
    /// Earlier attempts to carry in the first request: the previous artifact and why it fails now (R-SYNTH-46).
    pub feedback: Vec<AttemptFeedback>,
}

/// A candidate that passed every step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// The validated IR, with the compiler's `calls` joined in.
    pub ir: ValidIr,
    /// What it was verified on.
    pub verified: Verified,
}

/// A goal that ended with no artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Why: `VL0403`..`VL0408`, `VL0603` or `VL0607`.
    pub diagnostic: Diagnostic,
    /// One line per failed attempt, for `--verbose` (R-SYNTH-13).
    pub attempts: Vec<String>,
}

impl Failure {
    fn of(diagnostic: Diagnostic) -> Self {
        Failure {
            diagnostic,
            attempts: Vec::new(),
        }
    }
}

/// How synthesis of one goal ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A verified candidate.
    Built(Box<Built>),
    /// No artifact.
    Failed(Box<Failure>),
}

/// The `VL0404` of a goal the provider can't be reached for.
pub fn unavailable(goal: &str, span: Span, why: Option<&str>) -> Diagnostic {
    let mut d = Diagnostic::new(
        Code::ProviderUnavailable,
        span,
        format!("Velme couldn't reach the AI helper to build `{goal}`."),
    )
    .with_help("check the connection and try again, or build with another provider");
    if let Some(why) = why.filter(|w| !w.is_empty()) {
        d = d.with_note(why.to_owned());
    }
    d
}

/// The `VL0404` of a goal that needs building under `--offline`, which contacts no provider at all (`tooling/40`
/// R-CLI-05, D-106).
pub fn offline(goal: &str, span: Span) -> Diagnostic {
    Diagnostic::new(
        Code::ProviderUnavailable,
        span,
        format!("`{goal}` needs building, and `--offline` is on."),
    )
    .with_help("run `velme build` without `--offline`")
}

/// Synthesizes `task` with `provider`: at most `1 + max_retries` calls (R-SYNTH-11), each counted in `session`.
pub async fn synthesize(
    session: &mut Session,
    provider: &dyn SynthProvider,
    task: &Task<'_>,
    runner: &dyn ChildRunner,
) -> Outcome {
    match run(session, provider, task, runner).await {
        Ok(built) => Outcome::Built(Box::new(built)),
        Err(failure) => Outcome::Failed(Box::new(failure)),
    }
}

// A failure is rare and read once; boxing it would only add noise to every `?`.
#[allow(clippy::result_large_err)]
async fn run(
    session: &mut Session,
    provider: &dyn SynthProvider,
    task: &Task<'_>,
    runner: &dyn ChildRunner,
) -> Result<Built, Failure> {
    let internal = || Failure::of(Diagnostic::internal_error());
    let goal = task.program.goals.get(task.goal.0).ok_or_else(internal)?;
    let (name, span) = (goal.name.as_str(), goal.span);
    let options = session.options.clone();
    let base = build_request(task.program, task.goal, task.source).map_err(Failure::of)?;
    let compiler_calls = calls(task.program, task.goal).map_err(Failure::of)?;
    let mut history: Vec<Rejection> = Vec::new();
    let mut earlier: Vec<AttemptFeedback> = task.feedback.clone();
    // `reply_format` is a setting of the LLM providers (R-SYNTH-34..40): an external backend answers in canonical IR.
    let external = provider.id() == "external";
    let format = if external {
        ReplyFormat::IrJson
    } else {
        options.reply_format
    };
    for attempt in 0..=options.max_retries {
        if let Some(error) = &session.stopped {
            return Err(Failure::of(stopped_diagnostic(error, name, span)));
        }
        let cap = options.max_calls_per_build as usize;
        if session.calls >= cap {
            let d = Diagnostic::new(
                Code::SynthesisFailed,
                span,
                format!("Velme couldn't build `{name}`: this build reached its limit of {cap} provider calls."),
            )
            .with_note(format!("the limit is `max_calls_per_build` = {cap}"))
            .with_help("raise it in `[synthesis]`, or build fewer goals at once");
            return Err(Failure::of(d));
        }
        session.calls += 1;
        let mut request = base.clone();
        request.attempts = carried(&earlier, external, options.retry_history);
        let mut limits = SynthLimits::new(task.synthesis_key);
        limits.attempt = attempt;
        limits.timeout = std::time::Duration::from_secs(options.timeout_secs);
        limits.max_output_tokens = options
            .max_output_tokens
            .unwrap_or_else(|| provider.default_max_output_tokens());
        let reply = match provider.complete(&request, &limits).await {
            Ok(reply) => reply,
            Err(error) => match failed_call(session, error, provider.backend(), name, span, &history) {
                Ok(rejection) => {
                    history.push(rejection);
                    if let Some(end) = next(&mut earlier, &history, &options, attempt, name, span) {
                        return Err(end);
                    }
                    continue;
                }
                Err(failure) => return Err(failure),
            },
        };
        session.usage = add(session.usage, reply.usage);
        let rejection = match read(&reply.reply_json, format, &base) {
            Reply::Question(question) => return Err(Failure::of(question_diagnostic(name, span, &question))),
            Reply::Bad(rejection) => rejection,
            Reply::Internal => return Err(internal()),
            Reply::Candidate(text) => {
                let request = Request {
                    program: task.program,
                    goal: task.goal,
                    calls: &compiler_calls,
                    origin: Origin::Candidate,
                };
                match validate_detailed(&text, &request) {
                    Err(found) => Rejection::invalid(&reply.reply_json, &found, &base, format),
                    Ok(ir) => match verify(task.program, task.goal, task.source, task.contract_key, &ir, runner) {
                        Verdict::Accepted(verified) => return Ok(Built { ir, verified }),
                        Verdict::Rejected(mut rejection) => {
                            rejection.reply = reply.reply_json.clone();
                            rejection
                        }
                        Verdict::Watchdog(d) => {
                            let mut d = d;
                            d.span = span;
                            return Err(Failure::of(d));
                        }
                        Verdict::Internal => return Err(internal()),
                    },
                }
            }
        };
        history.push(rejection);
        if let Some(end) = next(&mut earlier, &history, &options, attempt, name, span) {
            return Err(end);
        }
    }
    Err(internal())
}

/// What follows a failed attempt: `Some` if the goal ends here, else the attempt is added to what the next request
/// carries.
fn next(
    earlier: &mut Vec<AttemptFeedback>,
    history: &[Rejection],
    options: &SynthOptions,
    attempt: u32,
    name: &str,
    span: Span,
) -> Option<Failure> {
    let verbose = || {
        history
            .iter()
            .enumerate()
            .map(|(i, r)| format!("attempt {}: {}", i + 1, r.verbose()))
            .collect()
    };
    let last = attempt >= options.max_retries;
    let repeat = options.stop_on_repeat
        && history.len() >= 2
        && matches!(history.iter().rev().map(|r| &r.cause).collect::<Vec<_>>().as_slice(), [a, b, ..] if a == b);
    if last || repeat {
        return Some(Failure {
            diagnostic: summary(name, history, repeat && !last, span),
            attempts: verbose(),
        });
    }
    if let Some(rejection) = history.last() {
        earlier.push(rejection.feedback());
    }
    None
}

/// The attempts a request carries (R-SYNTH-37): with `latest`, only the latest reply, and the primary diagnostic of each
/// earlier attempt; `external` always gets everything.
fn carried(earlier: &[AttemptFeedback], full: bool, history: RetryHistory) -> Vec<AttemptFeedback> {
    if full || history == RetryHistory::All || earlier.len() < 2 {
        return earlier.to_vec();
    }
    let split = earlier.len() - 1;
    earlier
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if i < split {
                AttemptFeedback {
                    reply: String::new(),
                    diagnostics: a.diagnostics.iter().take(1).cloned().collect(),
                }
            } else {
                a.clone()
            }
        })
        .collect()
}

/// A provider error: a failed attempt for `Refused` and `Malformed`, else the end of the goal (R-SYNTH-07).
#[allow(clippy::result_large_err)]
fn failed_call(
    session: &mut Session,
    error: ProviderError,
    backend: &str,
    name: &str,
    span: Span,
    history: &[Rejection],
) -> Result<Rejection, Failure> {
    match error {
        ProviderError::Refused(_) => Ok(Rejection::not_a_reply("", "was refused by the provider")),
        ProviderError::Malformed(_) => Ok(Rejection::not_a_reply("", "was not a usable reply")),
        ProviderError::Pending(text) => Err(Failure::of(pending(name, span, backend, &text, history))),
        other => {
            if reaches_no_further(&other) {
                session.stop(&other);
            }
            Err(Failure::of(provider_diagnostic(&other, backend, name, span)))
        }
    }
}

/// Whether the error means the provider can't be used, after which it is contacted no more (R-SYNTH-45): it can't be
/// reached, or it refuses what it was given.
pub fn reaches_no_further(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::Unavailable(_)
            | ProviderError::Timeout
            | ProviderError::RateLimited { .. }
            | ProviderError::NotConfigured
            | ProviderError::KeyRejected
            | ProviderError::TokenRejected { .. }
            | ProviderError::File { .. }
    )
}

/// What a goal that reaches a stopped provider ends with, without a request: `VL0405` or `VL0901` again if that is what
/// stopped it, else `VL0404` (R-SYNTH-45).
pub fn stopped_diagnostic(error: &ProviderError, name: &str, span: Span) -> Diagnostic {
    match error {
        ProviderError::NotConfigured
        | ProviderError::KeyRejected
        | ProviderError::TokenRejected { .. }
        | ProviderError::File { .. } => provider_diagnostic(error, "", name, span),
        _ => unavailable(name, span, None),
    }
}

/// The diagnostic of a provider error that ends a goal (R-SYNTH-07): also what the identity step and opening the provider
/// report. `Refused` and `Malformed` are failed attempts and never reach here from the loop; a bug if they do.
pub fn provider_diagnostic(error: &ProviderError, backend: &str, name: &str, span: Span) -> Diagnostic {
    match error {
        ProviderError::NotConfigured => Diagnostic::new(
            Code::ProviderNotConfigured,
            span,
            format!("I can't write `{name}` because no AI provider is set up."),
        )
        .with_help("set the provider, model and key it needs"),
        ProviderError::KeyRejected => Diagnostic::new(
            Code::ProviderNotConfigured,
            span,
            format!("I can't write `{name}` because the API key was rejected."),
        )
        .with_help("check the key in `VELME_API_KEY` (or `ANTHROPIC_API_KEY`), or set a valid one"),
        ProviderError::TokenRejected { sent: true } => Diagnostic::new(
            Code::ProviderNotConfigured,
            span,
            format!("I can't write `{name}` because the external backend rejected the token."),
        )
        .with_help("check the token in `VELME_EXTERNAL_TOKEN`, or unset it if the backend needs none"),
        ProviderError::TokenRejected { sent: false } => Diagnostic::new(
            Code::ProviderNotConfigured,
            span,
            format!("I can't write `{name}` because the external backend wants a token."),
        )
        .with_help("set `VELME_EXTERNAL_TOKEN` to the token the backend expects"),
        ProviderError::Unavailable(why) => unavailable(name, span, Some(why)),
        ProviderError::Timeout => unavailable(name, span, Some("the request timed out")),
        ProviderError::RateLimited { .. } => unavailable(name, span, Some("the provider is rate limiting requests")),
        ProviderError::BackendFailed { reason, body } => {
            let reason = clean_line(reason);
            let reason = if reason.is_empty() {
                "it failed".to_owned()
            } else {
                reason
            };
            let mut d = Diagnostic::new(
                Code::BackendFailed,
                span,
                format!("The backend `{backend}` couldn't build `{name}`: {reason}"),
            );
            if !body.is_empty() {
                d = d.with_note(format!("The backend's reply ended: {body}"));
            }
            d
        }
        ProviderError::Pending(text) => pending(name, span, backend, text, &[]),
        ProviderError::File { path, reason } => {
            Diagnostic::new(Code::FileError, span, format!("I couldn't open `{path}`.")).with_note(reason.clone())
        }
        ProviderError::Refused(_) | ProviderError::Malformed(_) | ProviderError::Internal(_) => {
            Diagnostic::internal_error()
        }
    }
}

/// `VL0408` for a queued request, or `VL0406` if its text is empty or too long (R-SYNTH-41).
fn pending(name: &str, span: Span, backend: &str, text: &str, history: &[Rejection]) -> Diagnostic {
    let Some(text) = clean_text(text) else {
        return Diagnostic::new(
            Code::BackendFailed,
            span,
            format!("The backend `{backend}` couldn't build `{name}`: it gave a waiting message I can't use."),
        );
    };
    let mut d = Diagnostic::new(
        Code::SynthesisPending,
        span,
        format!("`{name}` is waiting for an implementation from `{backend}`. Build again once it's ready."),
    )
    .with_note(format!("The backend said: \"{text}\""));
    if let Some(first) = history.first() {
        d = d.with_note(format!("An earlier attempt failed: {}", first.line));
    }
    d
}

fn question_diagnostic(name: &str, span: Span, question: &str) -> Diagnostic {
    Diagnostic::new(
        Code::PlanUnclear,
        span,
        format!(
            "Velme needs more detail to build `{name}`. Add the answer as an example, or to the plan, and build again."
        ),
    )
    .with_note(format!("The AI helper asked: \"{question}\""))
}

/// What a reply document is.
enum Reply {
    Question(String),
    /// The IR goal the reply's body completes, ready to validate (D-103).
    Candidate(String),
    Bad(Rejection),
    /// The goal couldn't be assembled: a bug of ours.
    Internal,
}

/// Reads a reply document (R-SYNTH-10, D-103): a question, a body to complete into IR and validate, or nothing usable.
/// Only `body` is read from it; whatever else it holds is not.
fn read(reply: &str, format: ReplyFormat, request: &SynthRequest) -> Reply {
    let value = match from_json_str::<serde_json::Value>(reply) {
        Ok(value) => value,
        Err(error) => return Reply::Bad(Rejection::unparsable(reply, &error)),
    };
    // The reply's own member names: a compact reply spells `body` with its alias (R-SYNTH-36).
    let body_key = match format {
        ReplyFormat::IrJson => Some("body"),
        ReplyFormat::Compact => compact::table().keys.get("body").map(String::as_str),
    };
    let body = body_key.and_then(|key| value.get(key));
    if let Some(map) = value.as_object()
        && map.contains_key("question")
    {
        if body.is_some() {
            return Reply::Bad(Rejection::not_a_reply(reply, "held both a body and a question"));
        }
        return match (
            map.len(),
            map.get("question").and_then(|q| q.as_str()).and_then(clean_text),
        ) {
            (1, Some(question)) => Reply::Question(question),
            _ => Reply::Bad(Rejection::not_a_reply(
                reply,
                "had a question that was empty, too long or not text",
            )),
        };
    }
    let Some(body) = body else {
        return Reply::Bad(Rejection::not_a_reply(reply, "had no `body`"));
    };
    let body = if format == ReplyFormat::IrJson {
        body.clone()
    } else {
        match compact::expand(body) {
            Ok(expanded) => expanded,
            Err(_) => {
                return Reply::Bad(Rejection::not_a_reply(
                    reply,
                    "used a short name that is not in the table",
                ));
            }
        }
    };
    // A body the provider chose can hold what canonical JSON refuses, such as a number out of range: a failed attempt,
    // not a bug of ours.
    if velme_ir::to_canonical_string(&body).is_err() {
        return Reply::Bad(Rejection::not_a_reply(reply, "holds a number Velme can't read"));
    }
    match request.assemble(&body) {
        Ok(text) => Reply::Candidate(text),
        Err(_) => Reply::Internal,
    }
}

fn add(a: Usage, b: Usage) -> Usage {
    Usage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_read_tokens: a.cache_read_tokens.saturating_add(b.cache_read_tokens),
        cache_write_tokens: a.cache_write_tokens.saturating_add(b.cache_write_tokens),
    }
}
