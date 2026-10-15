//! The retry loop (`compiler/22` §5): ask the provider for a candidate, validate it, verify it, and on failure ask again
//! with what Velme found, up to `max_retries` times. One goal at a time (R-SYNTH-01, D-93).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::{
    Fingerprint, MAX_JSON_DEPTH, Origin, Request, ValidIr, calls, from_json_str, from_json_str_within,
    validate_detailed,
};
use velme_sema::hir::{GoalId, Program};

use crate::attempt::{LogCause, Rejection, clean_line, clean_text, summary};
use crate::compact;
use crate::options::{ReplyFormat, RetryHistory, SynthOptions};
use crate::provider::{ProviderError, ReplyMeta, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::{AttemptFeedback, SynthRequest, build_request};
use crate::verify::{ChildRunner, Verdict, Verified, verify};

/// A source of wall-clock time for the synth log's `time` (`compiler/22` R-SYNTH-23): injected, so tests read the log
/// exactly.
pub trait WallClock: std::fmt::Debug + Send + Sync {
    /// Milliseconds since the Unix epoch, UTC.
    fn unix_millis(&self) -> i64;
}

/// The host's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn unix_millis(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

/// One provider request, as the synth log records it (R-SYNTH-23, D-109): what the runtime turns into a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    /// When the request was made, in milliseconds since the Unix epoch.
    pub time_millis: i64,
    /// The goal's attempt, from 0.
    pub attempt: u32,
    /// `ok`, or the `VL` code the attempt ended with.
    pub outcome: String,
    /// Why the attempt ended as it did; `None` exactly when `outcome` is `ok`.
    pub cause: Option<LogCause>,
    /// The provider's stop reason; `None` when the provider gave none or no response was read.
    pub stop_reason: Option<StopReason>,
    /// What the response looked like, `None` when none was read.
    pub shape: Option<ReplyShape>,
    /// Prompt-cache tokens read and written: `None` when the provider doesn't report them, `Some(0)` when it reports 0.
    pub cache: (Option<u64>, Option<u64>),
    /// Tokens sent and received: `None` for a count the provider didn't report or reported as 0.
    pub tokens: (Option<u64>, Option<u64>),
    /// How long the request took, as the provider measured it, else as this loop did.
    pub latency: Duration,
    /// The byte length of the reply as received; `None` when no reply came back.
    pub reply_bytes: Option<usize>,
    /// The reply format the request used, by its config name.
    pub reply_format: &'static str,
}

/// A provider's stop reason as the log may keep it: 1..=32 characters of `a`-`z` and `_`, else `"…"`. Only
/// the log builder makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReason(String);

impl StopReason {
    /// The text to write.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a reply looked like, and nothing it said (R-SYNTH-23, D-154): kinds and names from closed sets, `"…"` for the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyShape {
    /// The kinds of the response's content blocks in order, at most 64 and then one `"…"`; `None` for a
    /// provider without blocks.
    pub blocks: Option<Vec<String>>,
    /// The reply object's top-level keys that are an IR property name, a compact alias of one, `body`, `question`, `pending` or
    /// `error`, sorted and without repeats, then one `"…"` if any other key was there; empty if it isn't an object.
    pub keys: Vec<String>,
}

/// The mark for a value outside a closed set of the synth log.
const OTHER: &str = "…";

/// The most block kinds the log lists.
const MAX_BLOCKS: usize = 64;

/// The content block kinds the log names.
const BLOCK_KINDS: [&str; 4] = ["text", "tool_use", "thinking", "redacted_thinking"];

/// The reply members the log names besides the IR's property names and their aliases.
const REPLY_KEYS: [&str; 4] = ["body", "question", "pending", "error"];

fn stop_reason(raw: &str) -> StopReason {
    let closed = (1..=32).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    StopReason(if closed { raw.to_owned() } else { OTHER.to_owned() })
}

/// The shape of a response read: its block kinds, and the keys of its reply if it held one.
fn shape(meta: &ReplyMeta, reply: Option<&str>) -> ReplyShape {
    let blocks = meta.blocks.as_ref().map(|blocks| {
        let mut kinds: Vec<String> = blocks
            .iter()
            .take(MAX_BLOCKS)
            .map(|kind| {
                if BLOCK_KINDS.contains(&kind.as_str()) {
                    kind.clone()
                } else {
                    OTHER.to_owned()
                }
            })
            .collect();
        if blocks.len() > MAX_BLOCKS {
            kinds.push(OTHER.to_owned());
        }
        kinds
    });
    let table = compact::table();
    let known = |key: &str| {
        REPLY_KEYS.contains(&key) || table.keys.contains_key(key) || table.keys.values().any(|alias| alias == key)
    };
    let mut keys: Vec<String> = Vec::new();
    let mut other = false;
    if let Some(Ok(serde_json::Value::Object(map))) =
        reply.map(|r| from_json_str_within::<serde_json::Value>(r, MAX_JSON_DEPTH))
    {
        for key in map.keys() {
            if known(key) {
                keys.push(key.clone());
            } else {
                other = true;
            }
        }
    }
    keys.sort();
    keys.dedup();
    if other {
        keys.push(OTHER.to_owned());
    }
    ReplyShape { blocks, keys }
}

/// A response the log describes: a reply, or one that was read and held none.
struct Response<'a> {
    meta: &'a ReplyMeta,
    usage: Usage,
    latency: Duration,
    reply: Option<&'a str>,
}

impl<'a> Response<'a> {
    fn of(reply: &'a SynthReply) -> Self {
        Response {
            meta: &reply.meta,
            usage: reply.usage,
            latency: reply.latency,
            reply: Some(&reply.reply_json),
        }
    }
}

/// What one build's synthesis has done so far: the calls made and the tokens used, and the error that put the provider
/// out of reach, after which it is contacted no more (R-SYNTH-45, D-93).
#[derive(Debug, Clone)]
pub struct Session {
    options: SynthOptions,
    calls: usize,
    stopped: Option<ProviderError>,
    usage: Usage,
    clock: Arc<dyn WallClock>,
    records: Vec<CallRecord>,
}

impl Session {
    /// A session for one build.
    pub fn new(options: SynthOptions) -> Self {
        Session {
            options,
            calls: 0,
            stopped: None,
            usage: Usage::default(),
            clock: Arc::new(SystemWallClock),
            records: Vec::new(),
        }
    }

    /// The same session reading `clock` for the time of each request.
    pub fn with_wall_clock(mut self, clock: Arc<dyn WallClock>) -> Self {
        self.clock = clock;
        self
    }

    /// The requests made since the last call, oldest first, for the synth log.
    pub fn take_records(&mut self) -> Vec<CallRecord> {
        std::mem::take(&mut self.records)
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
    // An external backend (or a replay of one) is told the format when it is not `ir-json` (R-SYNTH-36, D-156).
    let external = provider.id() == "external";
    let format = options.reply_format;
    // The identity and the setting are one source of the format: a provider keyed for another would be read wrongly.
    if external && provider.input_version() != crate::request::external_input_version(format) {
        return Err(internal());
    }
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
        let mut request = if external {
            base.clone().for_external(format)
        } else {
            base.clone()
        };
        request.attempts = carried(&earlier, external, options.retry_history, format);
        let mut limits = SynthLimits::new(task.synthesis_key);
        limits.attempt = attempt;
        limits.timeout = std::time::Duration::from_secs(options.timeout_secs);
        limits.max_output_tokens = options
            .max_output_tokens
            .unwrap_or_else(|| provider.default_max_output_tokens());
        let (time_millis, started) = (session.clock.unix_millis(), Instant::now());
        let called = provider.call(&request, &limits).await;
        let record = |session: &mut Session, outcome: &str, cause: Option<LogCause>, response: Option<Response<'_>>| {
            let count = |n: fn(&Usage) -> u64| response.as_ref().map(|r| n(&r.usage)).filter(|&n| n > 0);
            let usage = response.as_ref().map(|r| r.usage);
            session.records.push(CallRecord {
                time_millis,
                attempt,
                outcome: outcome.to_owned(),
                cause,
                stop_reason: response
                    .as_ref()
                    .and_then(|r| r.meta.stop_reason.as_deref())
                    .map(stop_reason),
                shape: response.as_ref().map(|r| shape(r.meta, r.reply)),
                cache: usage.map_or((None, None), |u| (u.cache_read_tokens, u.cache_write_tokens)),
                tokens: (count(|u| u.input_tokens), count(|u| u.output_tokens)),
                latency: response.as_ref().map_or_else(|| started.elapsed(), |r| r.latency),
                reply_bytes: response.as_ref().and_then(|r| r.reply).map(str::len),
                reply_format: format.name(),
            });
        };
        let reply = match called.result {
            Ok(reply) => reply,
            Err(error) => {
                let variant = error.variant();
                match failed_call(session, error, provider.backend(), name, span, &history) {
                    Ok(rejection) => {
                        // A response that was read but held no reply still shows its stop reason, block kinds and
                        // billed tokens (R-SYNTH-21, R-SYNTH-23, D-155).
                        let seen = called.seen.as_ref().map(|seen| Response {
                            meta: &seen.meta,
                            usage: seen.usage,
                            latency: started.elapsed(),
                            reply: None,
                        });
                        if let Some(seen) = &called.seen {
                            session.usage = add(session.usage, seen.usage);
                        }
                        record(session, rejection.cause.code.as_str(), Some(rejection.log_cause), seen);
                        history.push(rejection);
                        if let Some(end) = next(&mut earlier, &history, &options, attempt, name, span) {
                            return Err(end);
                        }
                        continue;
                    }
                    Err(failure) => {
                        record(
                            session,
                            failure.diagnostic.code.as_str(),
                            Some(LogCause::Provider(variant)),
                            None,
                        );
                        return Err(failure);
                    }
                }
            }
        };
        session.usage = add(session.usage, reply.usage);
        let ir_request = Request {
            program: task.program,
            goal: task.goal,
            calls: &compiler_calls,
            origin: Origin::Candidate,
        };
        let rejection = match read(&reply.reply_json, format, &base, &ir_request) {
            Reply::Question(question) => {
                record(
                    session,
                    Code::PlanUnclear.as_str(),
                    Some(LogCause::Question),
                    Some(Response::of(&reply)),
                );
                return Err(Failure::of(question_diagnostic(name, span, &question)));
            }
            Reply::Bad(rejection) => {
                record(
                    session,
                    rejection.cause.code.as_str(),
                    Some(rejection.log_cause),
                    Some(Response::of(&reply)),
                );
                rejection
            }
            Reply::Internal => {
                record(
                    session,
                    Code::InternalError.as_str(),
                    Some(LogCause::Internal),
                    Some(Response::of(&reply)),
                );
                return Err(internal());
            }
            Reply::Candidate(text, notation) => {
                let request = Request {
                    program: task.program,
                    goal: task.goal,
                    calls: &compiler_calls,
                    origin: Origin::Candidate,
                };
                match validate_detailed(&text, &request) {
                    Err(found) => {
                        let mut rejection = Rejection::invalid(&reply.reply_json, &found, &base, format);
                        if let Some((body, spans)) = &notation {
                            rejection.notation_findings(body, spans, &found, &base);
                        }
                        record(
                            session,
                            rejection.cause.code.as_str(),
                            Some(rejection.log_cause),
                            Some(Response::of(&reply)),
                        );
                        rejection
                    }
                    Ok(ir) => match verify(task.program, task.goal, task.source, task.contract_key, &ir, runner) {
                        Verdict::Accepted(verified) => {
                            record(session, "ok", None, Some(Response::of(&reply)));
                            return Ok(Built { ir, verified });
                        }
                        Verdict::Rejected(mut rejection) => {
                            rejection.reply = reply.reply_json.clone();
                            record(
                                session,
                                rejection.cause.code.as_str(),
                                Some(rejection.log_cause),
                                Some(Response::of(&reply)),
                            );
                            rejection
                        }
                        Verdict::Watchdog(d) => {
                            let mut d = d;
                            d.span = span;
                            record(
                                session,
                                d.code.as_str(),
                                Some(LogCause::Runtime(d.code)),
                                Some(Response::of(&reply)),
                            );
                            return Err(Failure::of(d));
                        }
                        Verdict::Internal => {
                            record(
                                session,
                                Code::InternalError.as_str(),
                                Some(LogCause::Internal),
                                Some(Response::of(&reply)),
                            );
                            return Err(internal());
                        }
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
fn carried(
    earlier: &[AttemptFeedback],
    full: bool,
    history: RetryHistory,
    format: ReplyFormat,
) -> Vec<AttemptFeedback> {
    let split = earlier.len().saturating_sub(1);
    earlier
        .iter()
        .enumerate()
        .map(|(i, attempt)| {
            let mut attempt = attempt.clone();
            if i < split && !full && history == RetryHistory::Latest {
                attempt.reply.clear();
                attempt.diagnostics.truncate(1);
            }
            if i < split || format != ReplyFormat::Notation {
                for diagnostic in &mut attempt.diagnostics {
                    diagnostic.at = None;
                    diagnostic.quote.clear();
                }
            }
            attempt
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
        ProviderError::Refused(_) => Ok(Rejection::not_a_reply(
            "",
            "was refused by the provider",
            LogCause::Provider("refused"),
        )),
        ProviderError::Malformed(reason) => Ok(Rejection::malformed(reason)),
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
            | ProviderError::ModelMissing(_)
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
        | ProviderError::ModelMissing(_)
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
        ProviderError::ModelMissing(model) => Diagnostic::new(
            Code::ProviderNotConfigured,
            span,
            format!("The Ollama server doesn't have the model `{model}`."),
        )
        .with_help(format!(
            "run `ollama pull {model}`, or choose another model with `--model`"
        )),
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
        ProviderError::Rejected { status, message } => {
            let status = status.map(|status| format!(" (HTTP {status})")).unwrap_or_default();
            let mut d = Diagnostic::new(
                Code::ProviderNotConfigured,
                span,
                format!("I can't write `{name}` because the provider rejected the request{status}."),
            )
            .with_help("check that the model takes this kind of request, or choose another model with `--model`");
            let message = clean_line(message);
            if !message.is_empty() {
                d = d.with_note(format!("The provider said: \"{message}\""));
            }
            d
        }
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
        ProviderError::FormatUnsupported {
            backend,
            offered,
            asked,
        } => {
            let only = if offered.is_empty() {
                "none of the reply formats Velme knows".to_owned()
            } else {
                offered.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            };
            Diagnostic::new(
                Code::BackendFailed,
                span,
                format!(
                    "The backend `{backend}` couldn't build `{name}`: it answers only in {only}, and velme.toml asks for `reply_format = \"{asked}\"`."
                ),
            )
            .with_help("remove `reply_format` from `[synthesis]`, or use a backend that lists it")
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

/// Why a reply has no body: in `compact` it names the spelling asked for, since the canonical `body` isn't read (D-153).
fn no_body(body_key: &str) -> String {
    if body_key == "body" {
        "had no `body`".to_owned()
    } else {
        format!("had no `{body_key}`, the compact spelling of `body`")
    }
}

/// What a reply document is.
enum Reply {
    Question(String),
    /// The IR goal the reply's body completes, ready to validate (D-103).
    Candidate(String, Option<(String, crate::notation::Spans)>),
    Bad(Rejection),
    /// The goal couldn't be assembled: a bug of ours.
    Internal,
}

/// Reads a reply document (R-SYNTH-10, D-103): a question, a body to complete into IR and validate, or nothing usable.
/// Only `body` is read from it; whatever else it holds is not.
fn read(reply: &str, format: ReplyFormat, request: &SynthRequest, ir: &Request<'_>) -> Reply {
    let value = match from_json_str::<serde_json::Value>(reply) {
        Ok(value) => value,
        Err(error) => return Reply::Bad(Rejection::unparsable(reply, &error)),
    };
    // The reply's own member names: a compact reply spells `body` with its alias (R-SYNTH-36).
    let body_key = crate::prompt::body_key(format);
    let body = value.get(body_key);
    if let Some(map) = value.as_object()
        && map.contains_key("question")
    {
        if body.is_some() {
            return Reply::Bad(Rejection::not_a_reply(
                reply,
                "held both a body and a question",
                LogCause::Reply("body-and-question"),
            ));
        }
        return match (
            map.len(),
            map.get("question").and_then(|q| q.as_str()).and_then(clean_text),
        ) {
            (1, Some(question)) => Reply::Question(question),
            _ => Reply::Bad(Rejection::not_a_reply(
                reply,
                "had a question that was empty, too long or not text",
                LogCause::Reply("bad-question"),
            )),
        };
    }
    let Some(body) = body else {
        return Reply::Bad(Rejection::not_a_reply(
            reply,
            &no_body(body_key),
            LogCause::Reply("no-body"),
        ));
    };
    let mut notation = None;
    let body = match format {
        ReplyFormat::IrJson => body.clone(),
        ReplyFormat::Compact => match compact::expand(body) {
            Ok(expanded) => expanded,
            Err(_) => {
                return Reply::Bad(Rejection::not_a_reply(
                    reply,
                    "used a short name that is not in the table",
                    LogCause::Reply("unknown-alias"),
                ));
            }
        },
        ReplyFormat::Notation => {
            let Some(text) = body.as_str() else {
                return Reply::Bad(Rejection::not_a_reply(
                    reply,
                    "had a body that isn't a string",
                    LogCause::Reply("body-not-string"),
                ));
            };
            let lowered = match crate::notation::lower(text, request, ir) {
                Ok(lowered) => lowered,
                Err(rejected) => return Reply::Bad(Rejection::notation(reply, text, &rejected)),
            };
            notation = Some((text.to_owned(), lowered.spans));
            match serde_json::to_value(lowered.body) {
                Ok(body) => body,
                Err(_) => return Reply::Internal,
            }
        }
    };
    // A body the provider chose can hold what canonical JSON refuses, such as a number out of range: a failed attempt,
    // not a bug of ours.
    if velme_ir::to_canonical_string(&body).is_err() {
        return Reply::Bad(Rejection::not_a_reply(
            reply,
            "holds a number Velme can't read",
            LogCause::Reply("bad-number"),
        ));
    }
    match request.assemble(&body) {
        Ok(text) => Reply::Candidate(text, notation),
        Err(_) => Reply::Internal,
    }
}

fn add(a: Usage, b: Usage) -> Usage {
    // Not reported stays not reported until a call reports.
    let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    };
    Usage {
        input_tokens: a.input_tokens.saturating_add(b.input_tokens),
        output_tokens: a.output_tokens.saturating_add(b.output_tokens),
        cache_read_tokens: sum(a.cache_read_tokens, b.cache_read_tokens),
        cache_write_tokens: sum(a.cache_write_tokens, b.cache_write_tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_synth_60_only_the_latest_notation_diagnostics_carry_places() {
        let diagnostic = crate::AttemptDiagnostic {
            code: Code::IRSchemaInvalid.as_str().to_owned(),
            message: "test".to_owned(),
            path: None,
            detail: None,
            at: Some(crate::AttemptPlace { line: 1, column: 1 }),
            quote: vec!["body".to_owned()],
        };
        let attempts = vec![
            AttemptFeedback {
                reply: "reply".to_owned(),
                diagnostics: vec![diagnostic]
            };
            3
        ];
        for history in [RetryHistory::Latest, RetryHistory::All] {
            for full in [false, true] {
                for format in [ReplyFormat::IrJson, ReplyFormat::Compact, ReplyFormat::Notation] {
                    let carried = carried(&attempts, full, history, format);
                    for (index, attempt) in carried.iter().enumerate() {
                        let placed = index == 2 && format == ReplyFormat::Notation;
                        assert_eq!(attempt.diagnostics[0].at.is_some(), placed);
                        assert_eq!(!attempt.diagnostics[0].quote.is_empty(), placed);
                    }
                }
            }
        }
    }
}
