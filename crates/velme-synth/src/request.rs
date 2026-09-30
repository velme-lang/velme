//! The synthesis request (`compiler/22` §3.1, §4): the structured form of what a provider is asked, built from the
//! typed HIR of one goal alone (`compiler/20` R-CMP-06). Its JSON form is the `synthesize` message of the external
//! protocol (§3.2) and is described by the committed `velme-synth-request` schema.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use velme_builtins::{BUILTINS_VERSION, CATALOG, Shape, limits as caps};
use velme_check::{GoalChecks, lower_check};
use velme_diagnostics::{Diagnostic, Span};
use velme_ir::{CheckScope, Fingerprint, IR_VERSION, Type, encode_value, ir_type};
use velme_sema::hir::{self, GoalId, GoalKind, Program, Type as HirType, TypeId};

use crate::schema::reply_schema;

/// The version of the request document and of the external protocol (`compiler/22` §3.2). A change to either changes
/// the `input_version` of `external` backends and so every synthesis key.
pub const REQUEST_VERSION: &str = "0.1";

/// What is synthesized (`compiler/22` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// The whole `body` of a goal with no `call` block.
    Leaf,
    /// The tail `body` over the inputs and call bindings (D-5).
    Composite,
}

/// A goal parameter or record field, `name: Type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// Its name.
    pub name: String,
    /// Its type.
    #[serde(rename = "type")]
    pub ty: Type,
}

/// A goal's signature: `Name(params) -> Output`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    /// The goal's name.
    pub name: String,
    /// Its parameters, in declaration order.
    pub params: Vec<Param>,
    /// Its output type.
    pub output: Type,
}

/// A record type and its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordType {
    /// Its name.
    pub name: String,
    /// Its fields, in declaration order.
    pub fields: Vec<Param>,
}

/// A call binding of a composite goal, `name: Type = Child(args)`: the child's signature and never its IR (D-5, D-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalBinding {
    /// The bound name.
    pub name: String,
    /// The binding's type, the child's output.
    #[serde(rename = "type")]
    pub ty: Type,
    /// The child's signature.
    pub child: Signature,
    /// The arguments as written, one per child parameter.
    pub args: Vec<String>,
}

/// One `check` item: as written, and lowered to IR (`language/13` R-CHK-11), which is what runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckItem {
    /// The item's source text.
    pub source: String,
    /// Its lowered form: an IR expression node.
    pub ir: Value,
}

/// One `examples:` item, with literal values (D-7, R-SYNTH-38).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Example {
    /// The item's source text, `Double(2) == 4`.
    pub source: String,
    /// One input value per parameter, as JSON (D-23).
    pub args: Vec<Value>,
    /// The expected output, as JSON.
    pub expected: Value,
}

/// The effective budget of the goal's own invocation and the static caps (`runtime/30` §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Fuel for the invocation.
    pub max_fuel: u64,
    /// Bytes the invocation may allocate.
    pub max_memory: u64,
    /// Goal invocations in the goal's subtree.
    pub max_goal_calls: u64,
    /// Call depth below the goal.
    pub max_call_depth: u64,
    /// Items in any one list.
    pub max_list_size: u64,
    /// Bytes of the goal's output as JSON.
    pub max_output_bytes: u64,
}

/// One built-in a candidate may call (`language/14`), as the signatures of the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSig {
    /// Its name.
    pub name: String,
    /// One line per way of calling it: `(List<T>, T) -> Boolean`.
    pub signatures: Vec<String>,
}

/// A diagnostic shown to the provider on a retry (`compiler/22` R-SYNTH-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttemptDiagnostic {
    /// The `VLnnnn` code.
    pub code: String,
    /// Velme's message.
    pub message: String,
    /// The JSON path of the fault in the reply, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For a check or example failure: the input, the assertion and the actual values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An earlier attempt at the same goal: the reply and why it was refused (`compiler/22` §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttemptFeedback {
    /// The reply document as received.
    pub reply: String,
    /// What Velme found wrong with it, most important first (R-SYNTH-31).
    pub diagnostics: Vec<AttemptDiagnostic>,
}

/// Everything a provider is asked about one goal (`compiler/22` §3.1). Serialized, it is the `request` of the external
/// `synthesize` message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SynthRequest {
    /// [`REQUEST_VERSION`].
    pub request_version: String,
    /// The IR version the candidate must be written against (D-97).
    pub ir_version: String,
    /// The builtin catalog version the candidate must be written against (D-97).
    pub builtins_version: String,
    /// What to write.
    pub task: TaskKind,
    /// The goal's name.
    pub goal: String,
    /// The goal's signature.
    pub signature: Signature,
    /// Every record type reachable from the signature and the bindings, by name.
    pub types: Vec<RecordType>,
    /// The call bindings, for a composite goal.
    pub locals: Vec<LocalBinding>,
    /// The plan text, normalized (D-21). Untrusted user description (R-SYNTH-22).
    pub plan: String,
    /// The `check` items.
    pub checks: Vec<CheckItem>,
    /// The `examples:` items, in source order.
    pub examples: Vec<Example>,
    /// The effective budget.
    pub budget: Budget,
    /// The builtins a candidate may use.
    pub builtins: Vec<BuiltinSig>,
    /// Earlier attempts at this goal, oldest first.
    pub attempts: Vec<AttemptFeedback>,
    /// The JSON Schema the reply must satisfy: an IR goal or a question (R-SYNTH-10).
    pub output_schema: Value,
}

/// A message to an `external` backend (`compiler/22` §3.2): the document written to its stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalMessage {
    /// Asks the backend to name itself; answered once per build (R-SYNTH-26).
    Describe {
        /// [`REQUEST_VERSION`].
        request_version: String,
    },
    /// Asks for one candidate.
    Synthesize {
        /// [`REQUEST_VERSION`].
        request_version: String,
        /// The request.
        request: Box<SynthRequest>,
    },
}

impl ExternalMessage {
    /// The `describe` message.
    pub fn describe() -> Self {
        ExternalMessage::Describe {
            request_version: REQUEST_VERSION.to_owned(),
        }
    }

    /// The `synthesize` message for `request`.
    pub fn synthesize(request: SynthRequest) -> Self {
        ExternalMessage::Synthesize {
            request_version: REQUEST_VERSION.to_owned(),
            request: Box::new(request),
        }
    }
}

/// The JSON Schema of [`ExternalMessage`] at [`REQUEST_VERSION`] (`compiler/22` §3.2), generated from the Rust types
/// like the IR schema (`compiler/21` R-IR-20). The committed copy is `crates/velme-synth/schema/synth-request-0.1.json`.
pub fn request_schema() -> Value {
    schemars::schema_for!(ExternalMessage).to_value()
}

impl SynthRequest {
    /// The BLAKE3 of the request's canonical JSON (`compiler/21` R-IR-21), written `b3:<hex>`: what a replay fixture
    /// keeps of an exchange instead of the request (R-SYNTH-43).
    pub fn hash(&self) -> Result<Fingerprint, Diagnostic> {
        Fingerprint::of(self).map_err(|_| Diagnostic::internal_error())
    }
}

/// The request for `goal` of the checked `program`, whose source text is `source`. `goal` must be a leaf or composite
/// goal: a wired goal is never synthesized (D-4). The request has no earlier attempts.
pub fn build_request(program: &Program, goal: GoalId, source: &str) -> Result<SynthRequest, Diagnostic> {
    let hir = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    let task = match hir.kind {
        GoalKind::Leaf => TaskKind::Leaf,
        GoalKind::Composite => TaskKind::Composite,
        GoalKind::Wired => return Err(Diagnostic::internal_error()),
    };
    let scope = CheckScope::new(program, hir)?;
    let checks = hir
        .checks
        .iter()
        .map(|check| {
            let lowered = lower_check(program, hir, &scope, check)?;
            Ok(CheckItem {
                source: text(source, check.span)?.to_owned(),
                ir: serde_json::to_value(lowered.node.node()).map_err(|_| Diagnostic::internal_error())?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let cases = GoalChecks::new(program, goal, source)?;
    let examples = cases
        .examples()
        .iter()
        .map(|case| {
            Ok(Example {
                source: text(source, case.span)?.to_owned(),
                args: case.args.iter().map(json).collect::<Result<_, _>>()?,
                expected: json(&case.expected)?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let locals = hir
        .bindings
        .iter()
        .map(|binding| {
            let child = program
                .goals
                .get(binding.callee.0)
                .ok_or_else(Diagnostic::internal_error)?;
            Ok(LocalBinding {
                name: binding.name.clone(),
                ty: ir(program, &binding.ty)?,
                child: signature(program, child)?,
                args: binding
                    .args
                    .iter()
                    .map(|arg| text(source, arg.span).map(str::to_owned))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut roots: Vec<&HirType> = hir.params.iter().map(|p| &p.ty).chain([&hir.output]).collect();
    for binding in &hir.bindings {
        roots.push(&binding.ty);
        if let Some(child) = program.goals.get(binding.callee.0) {
            roots.extend(child.params.iter().map(|p| &p.ty).chain([&child.output]));
        }
    }
    let budget = hir.budget;
    Ok(SynthRequest {
        request_version: REQUEST_VERSION.to_owned(),
        ir_version: IR_VERSION.to_owned(),
        builtins_version: BUILTINS_VERSION.to_owned(),
        task,
        goal: hir.name.clone(),
        signature: signature(program, hir)?,
        types: record_types(program, roots)?,
        locals,
        plan: hir.plan.clone().unwrap_or_default(),
        checks,
        examples,
        budget: Budget {
            max_fuel: budget.max_fuel,
            max_memory: budget.max_memory,
            max_goal_calls: budget.max_goal_calls,
            max_call_depth: budget.max_call_depth,
            max_list_size: caps::MAX_LIST_SIZE,
            max_output_bytes: caps::MAX_OUTPUT_BYTES,
        },
        builtins: builtins(),
        attempts: Vec::new(),
        output_schema: reply_schema(),
    })
}

/// The builtins callable from synthesized IR, in catalog order (`compiler/22` §4, R-SYNTH-08).
pub fn builtins() -> Vec<BuiltinSig> {
    CATALOG
        .iter()
        .filter(|b| b.in_ir)
        .map(|b| BuiltinSig {
            name: b.name.to_owned(),
            signatures: b
                .signatures
                .iter()
                .map(|s| {
                    let params: Vec<String> = s.params.iter().map(shape).collect();
                    let line = format!("({}) -> {}", params.join(", "), shape(&s.output));
                    // A collection primitive is an IR node of its own kind; the validator rejects it as a `builtin`.
                    if b.function.is_none() {
                        format!("{line} (write as a `{}` node, not a builtin call)", b.name)
                    } else {
                        line
                    }
                })
                .collect(),
        })
        .collect()
}

/// `shape` as a learner reads it: `List<T>`, `Number?`, `(T) -> U`.
fn shape(shape: &Shape) -> String {
    match shape {
        Shape::Number => "Number".to_owned(),
        Shape::Text => "Text".to_owned(),
        Shape::Boolean => "Boolean".to_owned(),
        Shape::T => "T".to_owned(),
        Shape::U => "U".to_owned(),
        Shape::List(inner) => format!("List<{}>", self::shape(inner)),
        Shape::Optional(inner) => format!("{}?", self::shape(inner)),
        Shape::Lambda(params, output) => {
            let params: Vec<String> = params.iter().map(self::shape).collect();
            format!("({}) -> {}", params.join(", "), self::shape(output))
        }
    }
}

fn ir(program: &Program, ty: &HirType) -> Result<Type, Diagnostic> {
    ir_type(program, ty).ok_or_else(Diagnostic::internal_error)
}

fn signature(program: &Program, goal: &hir::Goal) -> Result<Signature, Diagnostic> {
    Ok(Signature {
        name: goal.name.clone(),
        params: goal
            .params
            .iter()
            .map(|p| {
                Ok(Param {
                    name: p.name.clone(),
                    ty: ir(program, &p.ty)?,
                })
            })
            .collect::<Result<_, Diagnostic>>()?,
        output: ir(program, &goal.output)?,
    })
}

/// Every record type reachable from `roots`, through fields too, by name.
fn record_types(program: &Program, roots: Vec<&HirType>) -> Result<Vec<RecordType>, Diagnostic> {
    let mut pending = Vec::new();
    for ty in roots {
        records_in(ty, &mut pending);
    }
    let mut found = BTreeMap::new();
    while let Some(id) = pending.pop() {
        let record = program.record(id).ok_or_else(Diagnostic::internal_error)?;
        if found.contains_key(&record.name) {
            continue;
        }
        let mut fields = Vec::new();
        for field in &record.fields {
            records_in(&field.ty, &mut pending);
            fields.push(Param {
                name: field.name.clone(),
                ty: ir(program, &field.ty)?,
            });
        }
        found.insert(
            record.name.clone(),
            RecordType {
                name: record.name.clone(),
                fields,
            },
        );
    }
    Ok(found.into_values().collect())
}

fn records_in(ty: &HirType, out: &mut Vec<TypeId>) {
    match ty {
        HirType::Optional(inner) | HirType::List(inner) => records_in(inner, out),
        HirType::Record(id) => out.push(*id),
        _ => {}
    }
}

/// The source text of `span`.
fn text(source: &str, span: Span) -> Result<&str, Diagnostic> {
    source.get(span.start..span.end).ok_or_else(Diagnostic::internal_error)
}

/// `value` as JSON (`language/11` R-TYP-23), numbers kept as written.
fn json(value: &velme_builtins::Value) -> Result<Value, Diagnostic> {
    let text = encode_value(value).map_err(|_| Diagnostic::internal_error())?;
    serde_json::from_str(&text).map_err(|_| Diagnostic::internal_error())
}
