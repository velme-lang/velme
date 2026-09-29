//! A goal and its inputs as the command line names them (`tooling/40` §3.1, D-23): one JSON object keyed by parameter
//! name, with single arguments overriding its keys, decoded against the goal's signature before anything runs
//! (`runtime/30` §4 step 2).

use std::collections::BTreeMap;
use std::io::Read;

use serde_json::Value as Json;
use velme_builtins::Value;
use velme_builtins::limits::{MAX_LIST_SIZE, MIB};
use velme_diagnostics::{Code, Diagnostic, Span, closest, did_you_mean};
use velme_ir::limits::MAX_DEPTH;
use velme_ir::{DecodeProblem, decode_value, from_json_str, json_kind};
use velme_sema::SourceFile;
use velme_sema::hir::{GoalId, Program};

/// The largest input document read (`tooling/40` R-CLI-07, T-9).
pub const MAX_INPUT_BYTES: u64 = 16 * MIB;

/// An input document from `reader`, which is named `name` if it can't be read (`tooling/40` §3.1). No more than
/// [`MAX_INPUT_BYTES`] and one byte are read: a longer document is `VL0902` before the rest is read (R-CLI-07, T-9).
/// Its problems have no place in the source.
pub fn read_input(reader: impl Read, name: &str) -> Result<String, Diagnostic> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| SourceFile::unreadable(name, &e))?;
    if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_INPUT_BYTES) {
        return Err(invalid(Span::default(), "The input is too big.".to_owned())
            .with_note(format!("at most {MAX_INPUT_BYTES} bytes are allowed")));
    }
    String::from_utf8(bytes).map_err(|_| {
        invalid(Span::default(), "The input isn't valid JSON.".to_owned()).with_note("it isn't UTF-8 text")
    })
}

/// The goal called `name` (`tooling/40` R-CLI-06): `VL0903` with the nearest spelling otherwise.
pub fn find_goal(program: &Program, name: &str) -> Result<GoalId, Diagnostic> {
    if let Some(i) = program.goals.iter().position(|g| g.name == name) {
        return Ok(GoalId(i));
    }
    let message = match closest(name, program.goals.iter().map(|g| g.name.as_str())) {
        Some(suggestion) => format!("There's no goal called `{name}`. Did you mean `{suggestion}`?"),
        None => format!("There's no goal called `{name}`."),
    };
    Err(Diagnostic::new(Code::GoalNotFound, Default::default(), message))
}

/// The inputs of `goal`, one per parameter in order, from the `--input` object `input` and the `--arg`s `args` (a
/// parameter name and its JSON text each), which override its keys. Every missing, extra or mistyped argument is
/// reported, naming the parameter, the type it needs and what it got (R-CLI-07), at the parameter it concerns or at the
/// goal.
pub fn decode_inputs(
    program: &Program,
    goal: GoalId,
    input: Option<&str>,
    args: &[(String, String)],
) -> Result<Vec<Value>, Vec<Diagnostic>> {
    let target = program
        .goals
        .get(goal.0)
        .ok_or_else(|| vec![Diagnostic::internal_error()])?;
    // Where a problem with the input as a whole points: the goal's name and inputs, not its whole body.
    let header = Span::new(
        target.span.start,
        target.params.last().map_or(target.span.start, |p| p.span.end),
    );
    let mut diags = Vec::new();
    let mut given: BTreeMap<String, Json> = BTreeMap::new();
    // One diagnostic per root cause: a value that couldn't be read isn't also missing (CC-ERR-04).
    let mut unread: Vec<&str> = Vec::new();
    let mut input_unread = false;
    if let Some(text) = input {
        input_unread = true;
        // The object holding the values is not a level of any value.
        match document(text, 1) {
            Ok(Json::Object(object)) => {
                given.extend(object);
                input_unread = false;
            }
            Ok(other) => diags.push(invalid(
                header,
                format!(
                    "The input should be a record with one field per input, but got {}.",
                    json_kind(&other)
                ),
            )),
            Err(reason) => diags.push(invalid(header, "The input isn't valid JSON.".to_owned()).with_note(reason)),
        }
    }
    for (name, text) in args {
        match document(text, 0) {
            Ok(json) => {
                given.insert(name.clone(), json);
            }
            Err(reason) => {
                unread.push(name);
                let span = target
                    .params
                    .iter()
                    .find(|p| &p.name == name)
                    .map_or(header, |p| p.span);
                diags.push(invalid(span, format!("Input `{name}` isn't valid JSON.")).with_note(reason));
            }
        }
    }
    let mut inputs = Vec::with_capacity(target.params.len());
    for param in &target.params {
        let ty = program.type_name(&param.ty);
        let Some(json) = given.remove(&param.name) else {
            if input_unread || unread.contains(&param.name.as_str()) {
                continue;
            }
            diags.push(
                invalid(
                    param.span,
                    format!("Input `{}` should be {ty}, but got no value.", param.name),
                )
                .with_help(format!("give it with `--arg {}=<json>`", param.name)),
            );
            continue;
        };
        match decode_value(&json, &param.ty, program) {
            Ok(value) => inputs.push(value),
            Err(error) => {
                let at = if error.path.is_empty() {
                    String::new()
                } else {
                    format!(" at `{}`", error.pointer())
                };
                let mut diag = match &error.problem {
                    // A list over `max_list_size` is too big, not mistyped (R-TYP-24).
                    DecodeProblem::TooManyItems { items } => Diagnostic::new(
                        error.code(),
                        param.span,
                        format!("`{}` made a list or answer that's too big.", target.name),
                    )
                    .with_note(format!(
                        "input `{}` has a list of {items} items{at}; at most {MAX_LIST_SIZE} are allowed",
                        param.name
                    )),
                    problem => {
                        let (expected, found) = match problem {
                            DecodeProblem::Mismatch { expected, found } => (expected.as_str(), (*found).to_owned()),
                            other => (ty.as_str(), got(other)),
                        };
                        let message = format!("Input `{}` should be {expected}, but got {found}.", param.name);
                        let diag = Diagnostic::new(error.code(), param.span, message);
                        if at.is_empty() {
                            diag
                        } else {
                            diag.with_note(at.trim_start().to_owned())
                        }
                    }
                };
                if let DecodeProblem::UnknownField { help: Some(help), .. } = &error.problem {
                    diag = diag.with_help(help.clone());
                }
                diags.push(diag);
            }
        }
    }
    for name in given.keys() {
        let diag = invalid(header, format!("`{}` has no input called `{name}`.", target.name));
        let help = did_you_mean(name, target.params.iter().map(|p| p.name.as_str()));
        diags.push(match help {
            Some(help) => diag.with_help(help),
            None => diag,
        });
    }
    if diags.is_empty() { Ok(inputs) } else { Err(diags) }
}

/// What JSON failing the mapping with `problem` holds, as the `found` of `reference/90`'s VL0902 message.
fn got(problem: &DecodeProblem) -> String {
    match problem {
        DecodeProblem::InvalidJson { .. } => "something that isn't valid JSON".to_owned(),
        DecodeProblem::Mismatch { found, .. } => (*found).to_owned(),
        DecodeProblem::NumberOutOfRange { number } => format!("{number}, which no Number holds exactly"),
        DecodeProblem::UnknownField { record, field, .. } => format!("a field `{field}` that `{record}` doesn't have"),
        DecodeProblem::MissingField { record, field } => format!("a `{record}` without the field `{field}`"),
        DecodeProblem::TooManyItems { items } => format!("a list of {items} items"),
    }
}

fn invalid(span: Span, message: String) -> Diagnostic {
    Diagnostic::new(Code::InvalidInput, span, message)
}

/// `text` as JSON, rejected before decoding if it is too big or if a value in it nests too deep (R-CLI-07), the values
/// being `outer` levels down; the reason otherwise.
fn document(text: &str, outer: usize) -> Result<Json, String> {
    if u64::try_from(text.len()).map_or(true, |len| len > MAX_INPUT_BYTES) {
        return Err(format!(
            "it is {} bytes long; at most {MAX_INPUT_BYTES} are allowed",
            text.len()
        ));
    }
    if nesting(text) > MAX_DEPTH + outer {
        return Err(format!("it nests deeper than {MAX_DEPTH} levels"));
    }
    from_json_str(text).map_err(|e| e.to_string())
}

/// How deep arrays and objects nest in JSON `text`, counted without parsing it.
fn nesting(text: &str) -> usize {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0usize, 0, false, false);
    for byte in text.bytes() {
        match (in_string, byte) {
            (true, _) if escaped => escaped = false,
            (true, b'\\') => escaped = true,
            (true, b'"') | (false, b'"') => in_string = !in_string,
            (false, b'[' | b'{') => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            (false, b']' | b'}') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}
