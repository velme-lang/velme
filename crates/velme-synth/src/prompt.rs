//! The prompt (`compiler/22` §4, D-97): a `SynthRequest` rendered through the versioned template of its task kind. Both
//! LLM providers share the templates; `external` sends the request as JSON instead.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde_json::{Map, Value, json};
use velme_diagnostics::Diagnostic;
use velme_ir::{Fingerprint, Type, to_canonical_string};

use crate::compact;
use crate::options::{PromptOptions, ReplyFormat, SchemaInPrompt};
use crate::request::{AttemptDiagnostic, RecordType, Signature, SynthRequest, TaskKind};
use crate::schema::{reply_schema, schema_summary};

/// A template's id: the task kind and the template's own version. Editing a template's text keeps its id and changes
/// its bytes, which `prompt_version` hashes.
const LEAF_ID: &str = "leaf-2";
const COMPOSITE_ID: &str = "composite-2";

const LEAF: &str = include_str!("../prompts/leaf.txt");
const COMPOSITE: &str = include_str!("../prompts/composite.txt");

/// Where the fixed prefix ends and the goal's own part begins, and where the retry turn's text begins.
const TASK_MARK: &str = "\n--- task ---\n";
const RETRY_MARK: &str = "\n--- retry ---\n";

/// Who speaks a turn of the conversation (`compiler/22` R-SYNTH-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Velme.
    User,
    /// The provider's earlier reply.
    Assistant,
}

/// One turn of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Who speaks.
    pub role: Role,
    /// What they say.
    pub text: String,
}

/// A rendered request: the first turn in two parts, then a turn pair per earlier attempt (R-SYNTH-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The header, output contract and allowed builtins: the same for every goal of one task kind (R-SYNTH-34), so the
    /// end of it is where `anthropic` puts a cache breakpoint.
    pub prefix: String,
    /// The rest of the first turn: what is specific to this goal.
    pub task: String,
    /// One assistant turn (the earlier reply) and one user turn (its diagnostics) for each earlier attempt.
    pub retries: Vec<Turn>,
}

impl Prompt {
    /// The whole first turn.
    pub fn first_turn(&self) -> String {
        format!("{}{}", self.prefix, self.task)
    }

    /// The conversation: the first turn, then the retry turns.
    pub fn turns(&self) -> Vec<Turn> {
        let mut turns = vec![Turn {
            role: Role::User,
            text: self.first_turn(),
        }];
        turns.extend(self.retries.iter().cloned());
        turns
    }
}

/// The version of everything that shapes the text sent (D-97): each template's id and bytes, and the schema summary
/// lines. `input_version` of an LLM provider starts with it (R-SYNTH-40). Editing any of them changes every
/// synthesis key.
pub fn prompt_version() -> String {
    static VERSION: LazyLock<String> = LazyLock::new(compute_version);
    VERSION.clone()
}

fn compute_version() -> String {
    let doc = json!({
        "leaf": {"id": LEAF_ID, "template": LEAF},
        "composite": {"id": COMPOSITE_ID, "template": COMPOSITE},
        "schema_summary": schema_summary(),
        "alias_table": compact::table(),
    });
    let hash = Fingerprint::of(&doc).map(|f| f.hex()).unwrap_or_default();
    format!("prompt-2:{hash}")
}

/// `request` rendered through its task kind's template with the default options.
pub fn render(request: &SynthRequest) -> Result<Prompt, Diagnostic> {
    render_with(request, &PromptOptions::default())
}

/// `request` rendered through its task kind's template, as `options` say (R-SYNTH-34..38). The header, output contract
/// and allowed builtins stay the same for every goal rendered with the same options (R-SYNTH-34).
pub fn render_with(request: &SynthRequest, options: &PromptOptions) -> Result<Prompt, Diagnostic> {
    let template = match request.task {
        TaskKind::Leaf => LEAF,
        TaskKind::Composite => COMPOSITE,
    };
    let (front, retry) = template.split_once(RETRY_MARK).ok_or_else(Diagnostic::internal_error)?;
    let (prefix, task) = front.split_once(TASK_MARK).ok_or_else(Diagnostic::internal_error)?;
    let fields = fields(request, options)?;
    let prefix = format!("{}\n", fill(prefix, &fields)?);
    let task = format!("{}\n", fill(task, &fields)?);
    let mut retries = Vec::new();
    // An earlier attempt whose reply is not repeated (`retry_history = "latest"`, R-SYNTH-37) is a line in the next user
    // turn instead: its primary diagnostic.
    let mut earlier: Vec<String> = Vec::new();
    for (n, attempt) in request.attempts.iter().enumerate() {
        if attempt.reply.is_empty() {
            let primary = attempt
                .diagnostics
                .first()
                .map(|d| diagnostics(std::slice::from_ref(d), options));
            earlier.push(format!(
                "Attempt {}, whose reply is not repeated:\n{}",
                n + 1,
                primary.unwrap_or_default()
            ));
            continue;
        }
        retries.push(Turn {
            role: Role::Assistant,
            text: shown_reply(&attempt.reply, options),
        });
        let mut own = BTreeMap::new();
        let mut found = std::mem::take(&mut earlier);
        found.push(diagnostics(&attempt.diagnostics, options));
        own.insert("diagnostics", found.join("\n"));
        retries.push(Turn {
            role: Role::User,
            text: fill(retry, &own)?,
        });
    }
    // Attempts with no reply of their own at the end (a refusal, or `latest` dropping replies) still owe their
    // diagnostics: they join the turn before them, so the conversation keeps alternating.
    let mut task = task;
    if !earlier.is_empty() {
        let mut own = BTreeMap::new();
        own.insert("diagnostics", earlier.join("\n"));
        let text = fill(retry, &own)?;
        match retries.last_mut() {
            Some(last) if last.role == Role::User => {
                last.text.push('\n');
                last.text.push_str(&text);
            }
            Some(_) => retries.push(Turn { role: Role::User, text }),
            None => {
                task.push('\n');
                task.push_str(&text);
                task.push('\n');
            }
        }
    }
    Ok(Prompt { prefix, task, retries })
}

/// `template` with each `{{name}}` replaced by its field. One pass: what a field holds is never read as a placeholder.
fn fill(template: &str, fields: &BTreeMap<&str, String>) -> Result<String, Diagnostic> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("{{") {
        let (name, tail) = after.split_once("}}").ok_or_else(Diagnostic::internal_error)?;
        out.push_str(before);
        out.push_str(fields.get(name).ok_or_else(Diagnostic::internal_error)?);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

/// An earlier reply as the provider is shown it: in the compact format when that is what it was asked for, if the reply
/// is JSON at all (R-SYNTH-36).
fn shown_reply(reply: &str, options: &PromptOptions) -> String {
    if options.reply_format == ReplyFormat::Compact
        && let Ok(value) = velme_ir::from_json_str::<Value>(reply)
        && let Ok(text) = to_canonical_string(&compact::compress(&value))
    {
        return text;
    }
    reply.to_owned()
}

fn fields(request: &SynthRequest, options: &PromptOptions) -> Result<BTreeMap<&'static str, String>, Diagnostic> {
    let mut fields = BTreeMap::new();
    let mut notes = String::new();
    if options.schema_in_prompt == SchemaInPrompt::Full {
        let schema = canonical(&reply_schema())?;
        notes.push_str(&format!("\nThe reply schema, as JSON Schema:\n{schema}"));
    }
    if options.reply_format == ReplyFormat::Compact {
        notes.push('\n');
        notes.push_str(&compact::describe());
    }
    fields.insert("format_notes", notes);
    fields.insert("prompt_version", prompt_version());
    fields.insert("ir_version", request.ir_version.clone());
    fields.insert("builtins_version", request.builtins_version.clone());
    fields.insert("schema_summary", schema_summary().join("\n"));
    fields.insert(
        "builtins",
        request
            .builtins
            .iter()
            .map(|b| format!("- {}: {}", b.name, b.signatures.join(" | ")))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    fields.insert("signature", signature(&request.signature));
    fields.insert("types", or_none(request.types.iter().map(record).collect()));
    fields.insert(
        "locals",
        or_none(
            request
                .locals
                .iter()
                .map(|l| {
                    format!(
                        "{}: {} = {}({})",
                        l.name,
                        type_name(&l.ty),
                        l.child.name,
                        l.args.join(", ")
                    )
                })
                .collect(),
        ),
    );
    fields.insert("plan", fenced(&request.plan));
    let checks: Result<Vec<String>, Diagnostic> = request
        .checks
        .iter()
        .map(|c| Ok(format!("{}\n  IR: {}", c.source, canonical(&c.ir)?)))
        .collect();
    fields.insert("checks", fenced(&or_none(checks?)));
    let examples: Result<Vec<String>, Diagnostic> = request
        .examples
        .iter()
        .take(options.max_prompt_examples)
        .map(|e| {
            let input: Map<String, Value> = request
                .signature
                .params
                .iter()
                .zip(&e.args)
                .map(|(p, v)| (p.name.clone(), v.clone()))
                .collect();
            Ok(format!(
                "{}\n  input: {}  expected: {}",
                e.source,
                canonical(&Value::Object(input))?,
                canonical(&e.expected)?
            ))
        })
        .collect();
    fields.insert("examples", fenced(&or_none(examples?)));
    let budget = &request.budget;
    fields.insert(
        "budget",
        format!(
            "fuel {}, memory {} bytes, {} goal calls, depth {}, lists of at most {} items, output of at most {} bytes",
            budget.max_fuel,
            budget.max_memory,
            budget.max_goal_calls,
            budget.max_call_depth,
            budget.max_list_size,
            budget.max_output_bytes
        ),
    );
    Ok(fields)
}

/// The diagnostics of one failed attempt as the retry turn lists them, their JSON paths in the requested format.
fn diagnostics(found: &[AttemptDiagnostic], options: &PromptOptions) -> String {
    found
        .iter()
        .map(|d| {
            let mut line = format!("- {}", d.code);
            if let Some(path) = &d.path {
                let path = if options.reply_format == ReplyFormat::Compact {
                    compact::compress_path(path)
                } else {
                    path.clone()
                };
                line.push_str(&format!(" at {path}"));
            }
            line.push_str(&format!(": {}", d.message));
            if let Some(detail) = &d.detail {
                line.push_str(&format!("\n  {detail}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn canonical(value: &Value) -> Result<String, Diagnostic> {
    to_canonical_string(value).map_err(|_| Diagnostic::internal_error())
}

fn or_none(lines: Vec<String>) -> String {
    if lines.is_empty() {
        "(none)".to_owned()
    } else {
        lines.join("\n")
    }
}

/// `text` in a fenced block whose fence is longer than any run of backticks inside it, so the text cannot close it
/// (`compiler/22` R-SYNTH-22).
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{text}\n{fence}")
}

fn signature(signature: &Signature) -> String {
    let params: Vec<String> = signature
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_name(&p.ty)))
        .collect();
    format!(
        "{}({}) -> {}",
        signature.name,
        params.join(", "),
        type_name(&signature.output)
    )
}

fn record(record: &RecordType) -> String {
    let fields: Vec<String> = record
        .fields
        .iter()
        .map(|f| format!("{}: {}", f.name, type_name(&f.ty)))
        .collect();
    format!("type {} = {{ {} }}", record.name, fields.join(", "))
}

/// `ty` as a learner writes it: `List<Player?>`.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Number {} => "Number".to_owned(),
        Type::Text {} => "Text".to_owned(),
        Type::Boolean {} => "Boolean".to_owned(),
        Type::Nothing {} => "Nothing".to_owned(),
        Type::Optional { of } => format!("{}?", type_name(of)),
        Type::List { of } => format!("List<{}>", type_name(of)),
        Type::Record { name } => name.clone(),
    }
}
