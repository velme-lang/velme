//! What one failed attempt is, for the retry loop and for the `VL0403` summary (`compiler/22` R-SYNTH-31): its cause, the
//! diagnostics the next request carries, and the line the learner reads. Everything a learner reads is composed here from
//! what Velme produced (codes, rule names, check and example source, inputs, computed values); reply text from the
//! provider never reaches it (R-SYNTH-22).

use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Invalid, ParseError, Subject};

use crate::compact;
use crate::options::ReplyFormat;
use crate::prompt::type_name;
use crate::request::{AttemptDiagnostic, AttemptFeedback, SynthRequest};

/// The most Unicode scalar values of a question or pending text (R-SYNTH-33).
pub(crate) const MAX_TEXT_CHARS: usize = 280;

/// What two attempts must share to have the same cause: the diagnostic's code and the check, example or validator rule
/// it names, whatever the JSON path or input (R-SYNTH-31, D-95).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause {
    /// The diagnostic's code.
    pub code: Code,
    /// The check, example or validator rule it names.
    pub name: String,
}

/// One failed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// What it shares with another attempt of the same cause.
    pub cause: Cause,
    /// What the learner reads for this cause, with its details (input, values) from this attempt.
    pub line: String,
    /// What `--verbose` lists for this attempt: Velme's words, with the JSON path or the input.
    pub verbose: String,
    /// For a failed case, the input as Velme worded it (`reference/90` VL0503's `{input}`); empty otherwise.
    pub input: String,
    /// The reply as received, or empty if there was none.
    pub reply: String,
    /// The diagnostics the next request carries, the primary one first (R-SYNTH-31).
    pub diagnostics: Vec<AttemptDiagnostic>,
}

impl Rejection {
    /// A reply that isn't a goal body or a question, or no reply at all: `VL0401` in Velme's own wording (R-SYNTH-10).
    pub(crate) fn not_a_reply(reply: &str, why: &str) -> Rejection {
        let message = format!("The reply wasn't a goal body or a question: {why}.");
        Rejection {
            cause: Cause {
                code: Code::IRSchemaInvalid,
                name: why.to_owned(),
            },
            line: format!("the reply {why}"),
            verbose: format!("{}: the reply {why}", Code::IRSchemaInvalid.as_str()),
            input: String::new(),
            reply: reply.to_owned(),
            diagnostics: vec![AttemptDiagnostic {
                code: Code::IRSchemaInvalid.as_str().to_owned(),
                message,
                path: None,
                detail: None,
            }],
        }
    }

    /// A reply that is JSON but breaks a rule of the JSON itself (`ParseError`: a repeated or reserved key, or nesting too
    /// deep): the validator's `schema-1`, at the reply's own JSON path, which is under `/body` (R-SYNTH-10, D-103).
    pub(crate) fn unparsable(reply: &str, error: &ParseError) -> Rejection {
        let (pointer, detail) = match error {
            ParseError::DuplicateKey { pointer } => (pointer.clone(), "this key appears twice".to_owned()),
            ParseError::ReservedKey { pointer } => (pointer.clone(), "this key is reserved".to_owned()),
            ParseError::TooDeep { pointer, limit } => (pointer.clone(), format!("it nests deeper than {limit} levels")),
            ParseError::Json(_) => return Rejection::not_a_reply(reply, "wasn't JSON"),
        };
        let code = Code::IRSchemaInvalid;
        let shown = safe_path(&pointer);
        let at = if shown.is_empty() {
            String::new()
        } else {
            format!(" at {shown}")
        };
        let line = format!("the reply {}{at}", rule_words("schema-1"));
        Rejection {
            cause: Cause {
                code,
                name: "schema/schema-1".to_owned(),
            },
            verbose: format!("{}: {line} (schema-1)", code.as_str()),
            input: String::new(),
            line,
            reply: reply.to_owned(),
            diagnostics: vec![AttemptDiagnostic {
                code: code.as_str().to_owned(),
                message: "The generated program wasn't in the right shape.".to_owned(),
                path: Some(pointer),
                detail: Some(detail),
            }],
        }
    }

    /// A candidate that failed validation (`compiler/21` §6): the first finding, in `R-CMP-16` order, is primary. What the
    /// learner reads is composed from the stage, the rule's id and the path, never from the finding's message, which can
    /// carry the candidate's own names (R-SYNTH-22); the next request carries the full detail.
    pub(crate) fn invalid(reply: &str, found: &[Invalid], request: &SynthRequest, format: ReplyFormat) -> Rejection {
        // A hint is said once per attempt, on the first finding that has it.
        let mut said: Vec<String> = Vec::new();
        let diagnostics: Vec<AttemptDiagnostic> = found
            .iter()
            .map(|f| {
                let mut sent = feedback(&f.diagnostic);
                if let Some(hint) = hint(f, request, format).filter(|h| !said.contains(h)) {
                    said.push(hint.clone());
                    sent.detail = Some(match sent.detail.take() {
                        Some(detail) => format!("{detail}; {hint}"),
                        None => hint,
                    });
                }
                sent
            })
            .collect();
        let (code, stage, rule, path) = found
            .first()
            .map_or((Code::IRInvalid, "internal", "internal-1", String::new()), |f| {
                (f.diagnostic.code, f.stage, f.rule, safe_path(&f.path))
            });
        let at = if path.is_empty() {
            String::new()
        } else {
            format!(" at {path}")
        };
        // What the learner reads is a one-line paraphrase of the rule; its id goes to `--verbose`, for tools and bug
        // reports (P-6).
        let line = format!("the reply {}{at}", rule_words(rule));
        Rejection {
            cause: Cause {
                code,
                name: format!("{stage}/{rule}"),
            },
            verbose: format!("{}: {line} ({rule})", code.as_str()),
            input: String::new(),
            line,
            reply: reply.to_owned(),
            diagnostics,
        }
    }

    /// The feedback an earlier attempt becomes in the next request.
    pub fn feedback(&self) -> AttemptFeedback {
        AttemptFeedback {
            reply: self.reply.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    /// What `--verbose` adds for this attempt.
    pub fn verbose(&self) -> &str {
        &self.verbose
    }

    /// What to suggest for this cause (`compiler/22` R-SYNTH-31's table).
    pub fn help(&self) -> &'static str {
        match self.cause.code {
            Code::ExampleFailed => "check the example, or say in the plan how that case is handled",
            Code::CheckFailed | Code::VerificationFailed => {
                "say in the plan what happens for that input, or narrow the check with `if … then …`"
            }
            Code::IRSchemaInvalid | Code::IRInvalid => match self.cause.name.split_once('/') {
                // A rule of the body's own content (`compiler/21` §6); a reply that wasn't a body at all has no rule.
                Some((_, "names-11" | "types-6" | "types-7" | "types-25" | "types-26")) => {
                    "the plan may need more than the listed builtins can do: simplify it or split the goal"
                }
                Some(("resources", _) | ("structure", "structure-1" | "structure-3")) => {
                    "the plan may be too big for one goal: simplify it or split the goal"
                }
                Some(("schema", _)) | None => {
                    "the AI helper's reply wasn't in a form Velme can use: build again, or try another model"
                }
                Some(_) => {
                    "the AI helper's code didn't fit the goal: build again, or add an example that shows the result"
                }
            },
            Code::CapabilityDenied => "goals can't use it: take the need out of the plan",
            Code::ArithmeticError => "say in the plan what should happen in that case",
            _ => "make the plan do less work per input, or split the goal",
        }
    }
}

/// The validator rule `rule` (`compiler/21` §6) in a learner's words, completing "the reply …". Only what the rule means:
/// never a name or value from the candidate (R-SYNTH-22).
fn rule_words(rule: &str) -> &'static str {
    match rule {
        "schema-1" => "isn't valid JSON of the shape the IR needs",
        "structure-1" | "structure-3" => "is longer than an IR program may be",
        "structure-2" | "structure-6" => "calls another goal itself, which only the goal's `call` block may do",
        "structure-4" => "is written for another version of the IR",
        "structure-5" => "uses one name twice in the same place",
        "names-1" => "is for a different goal than the one asked for",
        "names-2" | "names-3" | "names-5" | "types-3" => {
            "describes a record type the goal doesn't have, or describes one wrongly"
        }
        "names-6" => "uses a record type the goal doesn't have",
        "names-4" => "calls a goal the program doesn't have",
        "names-7" | "names-8" => "uses a name that isn't defined there",
        "names-9" | "names-10" => "uses a field its record doesn't have",
        "names-11" => "uses a built-in that doesn't exist",
        "types-1" | "types-2" | "types-5" => "gives a result of a different type than the goal promises",
        "types-4" => "takes inputs that differ from the goal's",
        "types-6" | "types-7" | "types-26" => "passes the wrong number or type of inputs",
        "types-8" => "holds a value that doesn't fit its type",
        "types-9" | "types-10" => "builds a record with the wrong fields",
        "types-11" => "puts an item in a list of another type",
        "types-12" | "types-13" => "reads a field from a value that may be nothing, or has none",
        "types-14" | "types-15" | "types-16" => "combines values of types that don't go together",
        "types-17" => "uses a condition that isn't true or false",
        "types-18" => "gives `if` branches of types that don't mix",
        "types-19" | "types-20" => "misuses a value that may be nothing",
        "types-21" | "types-22" | "types-23" | "types-24" => "misuses a list operation",
        "types-25" => "calls a list operation as a built-in",
        "callgraph-1" | "callgraph-2" => "lists calls that differ from the goal's `call` block",
        "resources-1" | "resources-2" | "resources-6" => "is too large or too deeply nested",
        "resources-3" | "resources-4" | "resources-5" => "holds a text or list that is too long",
        _ => "broke a rule of the IR",
    }
}

/// What the next request adds to a name or field the goal doesn't have (D-104, R-SYNTH-49): what is there, from the request
/// alone, and which node reads it, named as the reply is asked to spell it. The finding's `subject` only chooses the
/// words; nothing of the candidate's own text is repeated.
fn hint(found: &Invalid, request: &SynthRequest, format: ReplyFormat) -> Option<String> {
    let node = |kind: &str| match (format, compact::table().kinds.get(kind)) {
        (ReplyFormat::Compact, Some(alias)) => format!("`{kind}` (`{alias}`) node"),
        _ => format!("`{kind}` node"),
    };
    let list = |items: Vec<String>| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    let inputs = format!(
        "the goal's inputs, each read with an {}, are: {}",
        node("input"),
        list(
            request
                .signature
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, type_name(&p.ty)))
                .collect()
        )
    );
    // A composite's call bindings are names too (`compiler/22` R-SYNTH-49).
    let locals = if request.locals.is_empty() {
        String::new()
    } else {
        format!(
            ", and the call results, each read with a {}, are: {}",
            node("local"),
            list(
                request
                    .locals
                    .iter()
                    .map(|l| format!("{}: {}", l.name, type_name(&l.ty)))
                    .collect()
            )
        )
    };
    match (found.rule, &found.subject) {
        ("names-7", Subject::Name(_)) => Some(format!("{inputs}{locals}")),
        ("names-8", Subject::Name(name)) if name.contains('.') => {
            let head = name.split('.').next().unwrap_or_default();
            let base = if request.locals.iter().any(|l| l.name == head) {
                "local"
            } else {
                "input"
            };
            Some(format!(
                "a name is never dotted: read a field with a {} over the {}; {inputs}{locals}",
                node("field"),
                node(base)
            ))
        }
        ("names-8", Subject::Name(_)) => Some(format!(
            "{inputs}{locals}; a {} reads only a call result or a lambda's parameter",
            node("local")
        )),
        ("names-9" | "names-10", Subject::Record(name)) => {
            let record = request.types.iter().find(|r| &r.name == name)?;
            let fields = record
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, type_name(&f.ty)));
            Some(format!("the fields of `{name}` are: {}", list(fields.collect())))
        }
        ("types-15", Subject::TextArithmetic) => {
            Some("`add`, `sub`, `mul` and `div` are for Numbers only; join Text with the `concat` builtin".to_owned())
        }
        _ => None,
    }
}

/// A validator diagnostic as the next request carries it, in full: its code and message, the JSON path of its note, and
/// what the note says after the path.
fn feedback(diagnostic: &Diagnostic) -> AttemptDiagnostic {
    // serde's own errors end with a line and column of the document Velme assembled, which mean nothing in the reply.
    let without_position = |note: &str| -> String {
        match note.rsplit_once(" at line ") {
            Some((head, tail))
                if tail
                    .split_once(" column ")
                    .is_some_and(|(l, c)| l.bytes().chain(c.bytes()).all(|b| b.is_ascii_digit())) =>
            {
                head.to_owned()
            }
            _ => note.to_owned(),
        }
    };
    let mut path = None;
    let mut detail = Vec::new();
    for note in diagnostic.notes.iter().map(|n| without_position(n)) {
        let note = &note;
        match note.strip_prefix("at `").and_then(|rest| rest.split_once('`')) {
            Some((at, rest)) if path.is_none() => {
                path = Some(at.to_owned());
                let rest = rest.trim_start_matches(':').trim();
                if !rest.is_empty() {
                    detail.push(rest.to_owned());
                }
            }
            _ => detail.push(note.to_owned()),
        }
    }
    AttemptDiagnostic {
        code: diagnostic.code.as_str().to_owned(),
        message: diagnostic.message.clone(),
        path,
        detail: (!detail.is_empty()).then(|| detail.join("; ")),
    }
}

/// `path`, a JSON Pointer into a reply, with every segment that isn't an IR property name or an index replaced by `…`:
/// a segment can be a name the reply chose (R-SYNTH-22).
fn safe_path(path: &str) -> String {
    let table = crate::compact::table();
    path.split('/')
        .map(|segment| {
            if segment.is_empty() || segment.chars().all(|c| c.is_ascii_digit()) || table.keys.contains_key(segment) {
                segment
            } else {
                "…"
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `text` as an untrusted line (R-SYNTH-33): control characters, newlines and ANSI escapes become spaces, whitespace
/// runs collapse to one, and the ends are trimmed. `None` unless what is left is 1..=280 Unicode scalar values.
pub(crate) fn clean_text(text: &str) -> Option<String> {
    let collapsed = collapse(text);
    let length = collapsed.chars().count();
    (1..=MAX_TEXT_CHARS).contains(&length).then_some(collapsed)
}

/// `text` with escape sequences and control characters replaced by spaces and whitespace runs collapsed, in any length.
fn collapse(text: &str) -> String {
    let mut spaced = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                // A CSI sequence: parameters, then a final byte in `@`..`~`.
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                // An OSC sequence ends at BEL or ST (`ESC \`).
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
            spaced.push(' ');
        } else if c.is_control() {
            spaced.push(' ');
        } else {
            spaced.push(c);
        }
    }
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A backend's name or version (R-SYNTH-26): `None` if it holds any control, format (Unicode `Cf`) or bidi character,
/// which no honest name needs and which could reorder or hide what is shown; else whitespace runs collapse to one space
/// and the result must be 1..=`max` Unicode scalar values.
pub(crate) fn clean_name(text: &str, max: usize) -> Option<String> {
    if text
        .chars()
        .any(|c| c.is_control() || is_format(c) || velme_diagnostics::is_bidi_control(c))
    {
        return None;
    }
    let collapsed = collapse(text);
    (1..=max).contains(&collapsed.chars().count()).then_some(collapsed)
}

/// Whether `c` is in Unicode general category `Cf` (format), which `std` doesn't expose: invisible characters such as
/// zero-width spaces and joiners, the bidi controls, and the tag characters.
fn is_format(c: char) -> bool {
    matches!(u32::from(c),
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x890..=0x891 | 0x8e2 | 0x180e | 0x200b..=0x200f
        | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd
        | 0x13430..=0x1343f | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f)
}

/// The last `max` bytes of a backend's error output, cleaned as one line (R-SYNTH-28); a character cut by the start is
/// shown as U+FFFD.
pub(crate) fn clean_tail(bytes: &[u8], max: usize) -> String {
    let tail = bytes.get(bytes.len().saturating_sub(max)..).unwrap_or_default();
    collapse(&String::from_utf8_lossy(tail))
}

/// `text` as one untrusted line for a message (R-SYNTH-33's cleaning), cut to 280 scalar values instead of refused.
pub(crate) fn clean_line(text: &str) -> String {
    let collapsed = collapse(text);
    if collapsed.chars().count() > MAX_TEXT_CHARS {
        let cut: String = collapsed.chars().take(MAX_TEXT_CHARS).collect();
        format!("{cut}…")
    } else {
        collapsed
    }
}

/// `n` attempts, worded.
pub(crate) fn attempts(n: usize) -> String {
    if n == 1 {
        "1 attempt".to_owned()
    } else {
        format!("{n} attempts")
    }
}

/// The `VL0403` for a goal whose attempts `history` all failed (R-SYNTH-13, R-SYNTH-31): the cause shared by the most
/// attempts, ties going to the later one, with its details from the latest attempt with that cause; each other cause is
/// one note, in first-seen order. `stopped` is set when the loop ended early on a repeat (R-SYNTH-37).
pub(crate) fn summary(goal: &str, history: &[Rejection], stopped: bool, span: velme_diagnostics::Span) -> Diagnostic {
    let mut causes: Vec<(&Cause, usize, usize)> = Vec::new();
    for (i, attempt) in history.iter().enumerate() {
        match causes.iter_mut().find(|(cause, _, _)| **cause == attempt.cause) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 = i;
            }
            None => causes.push((&attempt.cause, 1, i)),
        }
    }
    let n = history.len();
    let Some(&(_, shared, latest)) = causes.iter().max_by_key(|(_, count, last)| (*count, *last)) else {
        return Diagnostic::internal_error();
    };
    let Some(main) = history.get(latest) else {
        return Diagnostic::internal_error();
    };
    // `reference/90`'s VL0403 wording; a loop that stopped early on a repeat says so after the count.
    let tries = if stopped {
        format!("{shared} of {n} tries, stopped after {}", attempts(n))
    } else {
        format!("{shared} of {n} tries")
    };
    let message = format!("Velme couldn't build `{goal}`: {} ({tries}).", main.line);
    let mut diagnostic = Diagnostic::new(Code::SynthesisFailed, span, message);
    for (cause, _, last) in &causes {
        if **cause != main.cause
            && let Some(other) = history.get(*last)
        {
            diagnostic = diagnostic.with_note(format!("Another attempt failed differently: {}", other.line));
        }
    }
    let help = if stopped {
        format!("{}; `stop_on_repeat = false` retries anyway", main.help())
    } else {
        main.help().to_owned()
    };
    diagnostic.with_help(help)
}

#[cfg(test)]
mod tests {
    use super::rule_words;

    /// Every rule id of the validator (`compiler/21` §6) has words of its own (they are fixed text, so none can quote the candidate).
    #[test]
    fn every_validator_rule_has_a_learner_sentence() {
        let stages: [(&str, u32); 6] = [
            ("schema", 1),
            ("structure", 6),
            ("names", 11),
            ("types", 26),
            ("callgraph", 2),
            ("resources", 6),
        ];
        for (stage, count) in stages {
            for n in 1..=count {
                let words = rule_words(&format!("{stage}-{n}"));
                assert_ne!(words, rule_words("no-such-rule"), "{stage}-{n}");
            }
        }
    }
}
