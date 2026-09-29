//! The trace (`runtime/30` §8): the tree of what a run did, made from its [`GoalRun`] records, ordered by source order
//! and never by completion time. Everything in it but the `*_us` timing fields is the same for every `--jobs` and every
//! run (R-RUN-12); comparisons ignore those.

use std::fmt::Write as _;
use std::time::Duration;

use serde::Serialize;
use serde::ser::{Error as _, SerializeMap, SerializeSeq as _, Serializer};
use serde_json::Value as Json;
use velme_builtins::Value;
use velme_ir::{display_value, encode_value};

use crate::sched::{CallRun, CallStatus, CheckRun, GoalRun};

/// The version of the trace JSON, which changes only by adding fields (R-RUN-19).
pub const TRACE_VERSION: u32 = 1;

/// A value that serializes as JSON by the one mapping (D-23, `language/11` R-TYP-23): records' fields in declaration
/// order, numbers as R-TYP-08 spells them. (A `serde_json::Value` would sort the fields.) Canonical JSON for hashing
/// stays sorted and is `velme-ir`'s.
#[derive(Debug, Clone, Copy)]
pub struct OrderedValue<'a>(pub &'a Value);

impl Serialize for OrderedValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Number(n) => {
                let number: serde_json::Number = n.to_string().parse().map_err(S::Error::custom)?;
                number.serialize(serializer)
            }
            Value::Text(text) => serializer.serialize_str(text),
            Value::Boolean(b) => serializer.serialize_bool(*b),
            Value::Nothing => serializer.serialize_none(),
            Value::List(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items.iter() {
                    seq.serialize_element(&OrderedValue(item))?;
                }
                seq.end()
            }
            Value::Record(record) => {
                let mut map = serializer.serialize_map(Some(record.fields.len()))?;
                for (name, item) in &record.fields {
                    map.serialize_entry(name, &OrderedValue(item))?;
                }
                map.end()
            }
        }
    }
}

/// The run's trace as a serializable tree (`velme trace --json`, `tooling/40` §3.2): `version`, `file`
/// (project-relative, R-CLI-19), `reproducible` (D-10) and the root `goal` event, with its `call` events nested in
/// source order. Only the `start_us`, `end_us` and `duration_us` fields differ between runs. A value past
/// `max_output_bytes` is left out and `"truncated": true` stands in its place (R-RUN-20).
#[derive(Debug, Clone, Copy)]
pub struct Trace<'a> {
    run: &'a GoalRun,
    file: &'a str,
}

impl Trace<'_> {
    /// The trace as a `serde_json::Value`, whose object keys are sorted: for looking at, not for output.
    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or(Json::Null)
    }
}

impl GoalRun {
    /// This run's trace, for `file` (project-relative).
    pub fn trace<'a>(&'a self, file: &'a str) -> Trace<'a> {
        Trace { run: self, file }
    }

    /// The trace for a person, indented by nesting and without timings (`runtime/30` R-RUN-19, R-RUN-20).
    pub fn trace_text(&self) -> String {
        let mut out = String::new();
        if !self.reproducible() {
            out.push_str("This run isn't reproducible: it was stopped by the clock.\n");
        }
        goal_text(self, 0, &mut out);
        out
    }

    /// Hands this run to `write` to be stored — as a cached result, a verification verdict or a saved trace — unless it
    /// isn't reproducible, when nothing is written and `None` is returned (`runtime/30` R-RUN-14, D-10). Every such
    /// write goes through here.
    pub fn persist_if_reproducible<T>(&self, write: impl FnOnce(&GoalRun) -> T) -> Option<T> {
        self.reproducible().then(|| write(self))
    }
}

impl Serialize for Trace<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("version", &TRACE_VERSION)?;
        map.serialize_entry("file", self.file)?;
        map.serialize_entry("reproducible", &self.run.reproducible())?;
        map.serialize_entry("goal", &GoalEvent(self.run))?;
        map.end()
    }
}

/// Microseconds, saturating.
fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// The `value` entry of an event, or `truncated` if the value is past `max_output_bytes` (R-RUN-20): never a `null`,
/// which is what `nothing` is.
fn entry<M: SerializeMap>(map: &mut M, value: &Value) -> Result<(), M::Error> {
    if encode_value(value).is_ok() {
        map.serialize_entry("value", &OrderedValue(value))
    } else {
        map.serialize_entry("truncated", &true)
    }
}

/// A value in a list of an event, with its name if it has one.
struct Slot<'a>(Option<&'a str>, &'a Value);

impl Serialize for Slot<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(name) = self.0 {
            map.serialize_entry("name", name)?;
        }
        entry(&mut map, self.1)?;
        map.end()
    }
}

/// A serializable view of one event of the trace.
enum Event<'a> {
    Call(&'a CallRun),
    Check(&'a CheckRun),
    Slot(Slot<'a>),
}

impl Serialize for Event<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Event::Call(call) => call_event(call, serializer),
            Event::Check(check) => check_event(check, serializer),
            Event::Slot(slot) => slot.serialize(serializer),
        }
    }
}

struct GoalEvent<'a>(&'a GoalRun);

impl Serialize for GoalEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let run = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("goal", &run.goal)?;
        map.serialize_entry("artifact", &run.artifact.map(|a| a.to_string()))?;
        map.serialize_entry("kind", &run.kind)?;
        let inputs: Vec<Event<'_>> = run
            .inputs
            .iter()
            .map(|(name, value)| Event::Slot(Slot(Some(name), value)))
            .collect();
        map.serialize_entry("inputs", &inputs)?;
        match &run.outcome {
            Ok(value) => {
                map.serialize_entry("outcome", "ok")?;
                entry(&mut map, value)?;
            }
            Err(failed) => {
                map.serialize_entry("outcome", "failed")?;
                if let Some(first) = failed.diagnostics().first() {
                    map.serialize_entry(
                        "failure",
                        &Failure {
                            code: first.code.as_str(),
                            message: &first.message,
                            path: failed.path(),
                        },
                    )?;
                }
            }
        }
        map.serialize_entry("fuel", &run.fuel)?;
        map.serialize_entry("memory", &run.memory)?;
        map.serialize_entry("start_us", &micros(run.timing.start))?;
        map.serialize_entry("end_us", &micros(run.timing.end))?;
        map.serialize_entry("duration_us", &micros(run.timing.end.saturating_sub(run.timing.start)))?;
        let calls: Vec<Event<'_>> = run.calls.iter().map(Event::Call).collect();
        map.serialize_entry("calls", &calls)?;
        let checks: Vec<Event<'_>> = run.checks.iter().map(Event::Check).collect();
        map.serialize_entry("checks", &checks)?;
        map.end()
    }
}

#[derive(Serialize)]
struct Failure<'a> {
    code: &'a str,
    message: &'a str,
    path: &'a [String],
}

fn call_event<S: Serializer>(call: &CallRun, serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(None)?;
    map.serialize_entry("binding", &call.binding)?;
    map.serialize_entry("goal", &call.callee)?;
    map.serialize_entry("wave", &call.wave)?;
    let args: Vec<Event<'_>> = call.args.iter().map(|arg| Event::Slot(Slot(None, arg))).collect();
    map.serialize_entry("args", &args)?;
    map.serialize_entry("outcome", status(call.status()))?;
    if let Some(run) = &call.run {
        if let Ok(value) = &run.outcome {
            entry(&mut map, value)?;
        }
        map.serialize_entry("run", &GoalEvent(run))?;
    }
    map.end()
}

fn check_event<S: Serializer>(check: &CheckRun, serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(None)?;
    map.serialize_entry("text", &check.text)?;
    map.serialize_entry(
        "span",
        &Span {
            start: check.span.start,
            end: check.span.end,
        },
    )?;
    map.serialize_entry("passed", &check.passed)?;
    let values: Vec<Part<'_>> = check.values.iter().map(|p| Part(&p.text, p.value.as_ref())).collect();
    map.serialize_entry("values", &values)?;
    map.end()
}

#[derive(Serialize)]
struct Span {
    start: usize,
    end: usize,
}

/// A path or helper call of a failed check and its value, if it was evaluated.
struct Part<'a>(&'a str, Option<&'a Value>);

impl Serialize for Part<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("text", self.0)?;
        if let Some(value) = self.1 {
            entry(&mut map, value)?;
        }
        map.end()
    }
}

fn status(status: CallStatus) -> &'static str {
    match status {
        CallStatus::Ok => "ok",
        CallStatus::Failed => "failed",
        CallStatus::Skipped => "skipped",
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

/// One goal event and everything under it, `depth` levels in. Values are cut as R-RUN-20 says.
fn goal_text(run: &GoalRun, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    let kind = serde_json::to_value(run.kind)
        .ok()
        .and_then(|k| k.as_str().map(str::to_owned))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "{pad}{}  {kind}  {}  fuel {}, memory {} bytes",
        run.goal,
        mark(run.outcome.is_ok()),
        run.fuel,
        run.memory
    );
    if let Some(artifact) = run.artifact {
        let _ = writeln!(out, "{pad}  artifact {artifact}");
    }
    for (name, value) in &run.inputs {
        let _ = writeln!(out, "{pad}  input {name} = {}", display_value(value));
    }
    for call in &run.calls {
        let shown = match (&call.run, call.status()) {
            (Some(GoalRun { outcome: Ok(value), .. }), _) => format!("{}  {}", mark(true), display_value(value)),
            (Some(_), _) => mark(false).to_owned(),
            (None, _) => "skipped".to_owned(),
        };
        let args: Vec<String> = call.args.iter().map(display_value).collect();
        let _ = writeln!(
            out,
            "{pad}  wave {}: {} = {}({})  {shown}",
            call.wave,
            call.binding,
            call.callee,
            args.join(", ")
        );
        if let Some(child) = &call.run {
            goal_text(child, depth + 2, out);
        }
    }
    for check in &run.checks {
        let _ = writeln!(out, "{pad}  check `{}`  {}", check.text, mark(check.passed));
        for part in &check.values {
            match &part.value {
                Some(value) => {
                    let _ = writeln!(out, "{pad}    `{}` = {}", part.text, display_value(value));
                }
                None => {
                    let _ = writeln!(out, "{pad}    `{}` was not evaluated", part.text);
                }
            }
        }
    }
    if let Err(failed) = &run.outcome
        && let Some(first) = failed.diagnostics().first()
    {
        let _ = writeln!(
            out,
            "{pad}  {}  [{}]  ({})",
            first.message,
            first.code.as_str(),
            failed.path().join(" › ")
        );
    }
}
