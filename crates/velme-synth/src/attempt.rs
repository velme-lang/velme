//! What one failed attempt is, for the retry loop and for the `VL0403` summary (`compiler/22` R-SYNTH-31): its cause, the
//! diagnostics the next request carries, and the line the learner reads. Everything a learner reads is composed here from
//! what Velme produced (codes, rule names, check and example source, inputs, computed values); reply text from the
//! provider never reaches it (R-SYNTH-22).

use velme_diagnostics::{Code, Diagnostic};
use velme_ir::Invalid;

use crate::request::{AttemptDiagnostic, AttemptFeedback};

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
    /// A reply that isn't an IR goal or a question, or no reply at all: `VL0401` in Velme's own wording (R-SYNTH-10).
    pub(crate) fn not_a_reply(reply: &str, why: &str) -> Rejection {
        let message = format!("The reply wasn't an IR goal or a question: {why}.");
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

    /// A candidate that failed validation (`compiler/21` §6): the first finding, in `R-CMP-16` order, is primary. What the
    /// learner reads is composed from the stage, the rule's id and the path, never from the finding's message, which can
    /// carry the candidate's own names (R-SYNTH-22); the next request carries the full detail.
    pub(crate) fn invalid(reply: &str, found: &[Invalid]) -> Rejection {
        let diagnostics: Vec<AttemptDiagnostic> = found.iter().map(|f| feedback(&f.diagnostic)).collect();
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
        let line = if stage == "schema" {
            format!("the reply isn't valid JSON of the shape the IR needs{at}")
        } else {
            format!("the reply broke the IR {stage} rule {rule}{at}")
        };
        Rejection {
            cause: Cause {
                code,
                name: format!("{stage}/{rule}"),
            },
            verbose: format!("{}: {line}", code.as_str()),
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
            Code::IRSchemaInvalid | Code::IRInvalid => {
                "the plan may need more than the listed builtins can do: simplify it or split the goal"
            }
            Code::CapabilityDenied => "goals can't use it: take the need out of the plan",
            Code::ArithmeticError => "say in the plan what should happen in that case",
            _ => "make the plan do less work per input, or split the goal",
        }
    }
}

/// A validator diagnostic as the next request carries it, in full: its code and message, the JSON path of its note, and
/// what the note says after the path.
fn feedback(diagnostic: &Diagnostic) -> AttemptDiagnostic {
    let mut path = None;
    let mut detail = Vec::new();
    for note in &diagnostic.notes {
        match note.strip_prefix("at `").and_then(|rest| rest.split_once('`')) {
            Some((at, rest)) if path.is_none() => {
                path = Some(at.to_owned());
                let rest = rest.trim_start_matches(':').trim();
                if !rest.is_empty() {
                    detail.push(rest.to_owned());
                }
            }
            _ => detail.push(note.clone()),
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
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = collapsed.chars().count();
    (1..=MAX_TEXT_CHARS).contains(&length).then_some(collapsed)
}

/// `text` as one untrusted line for a message (R-SYNTH-33's cleaning), cut to 280 scalar values instead of refused.
pub(crate) fn clean_line(text: &str) -> String {
    let collapsed: String = text.split(|c: char| c.is_control()).collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
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
