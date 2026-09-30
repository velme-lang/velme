//! Fingerprints (`runtime/32` §2, D-11, D-26): BLAKE3 over canonical JSON (D-21), written `b3:<hex>`.
//!
//! Each key hashes a small JSON document built here, never a Rust type's derived serialization, so a compiler refactor
//! cannot change a key (R-ART-04): operators are their surface spelling, and the only serialized types are the IR's own
//! (`Type`, `RecordType`), whose form is the versioned IR schema (`compiler/21` R-IR-20). The goal source enters as
//! normalized structure (R-ART-23, D-81).
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value as Json, json};
use velme_builtins::BUILTINS_VERSION;
use velme_builtins::limits::{MAX_LIST_SIZE, MAX_OUTPUT_BYTES};
use velme_diagnostics::Diagnostic;
use velme_sema::hir::{self, Expr, ExprKind, GoalId, Program, Type as HirType, TypeId};

use crate::lower::{ir_type, number};
use crate::node::RecordType;
use crate::{CanonicalError, IR_VERSION, to_canonical_string};

/// How a fingerprint is written: `b3:` then 64 lowercase hex digits (`runtime/32` §2).
const PREFIX: &str = "b3:";

/// A BLAKE3 hash, written `b3:<hex>`: a signature, a key, an artifact address or an execution id (`runtime/32` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint([u8; blake3::OUT_LEN]);

impl Fingerprint {
    /// The hash of `bytes`, e.g. of an artifact file (R-ART-10).
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Fingerprint(*blake3::hash(bytes).as_bytes())
    }

    /// The hash of `value`'s canonical JSON (R-IR-21).
    pub fn of<T: Serialize + ?Sized>(value: &T) -> Result<Self, CanonicalError> {
        to_canonical_string(value).map(|text| Self::of_bytes(text.as_bytes()))
    }

    /// The 64 hex digits, without `b3:`.
    pub fn hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", self.hex())
    }
}

/// Text that is not a `b3:` fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintError(String);

impl fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not `{PREFIX}` followed by 64 lowercase hex digits", self.0)
    }
}

impl std::error::Error for FingerprintError {}

impl FromStr for Fingerprint {
    type Err = FingerprintError;

    /// Reads `b3:<hex>`. Only the lowercase spelling is accepted, so each fingerprint has one written form.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.strip_prefix(PREFIX)
            .filter(|hex| hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .and_then(|hex| blake3::Hash::from_hex(hex).ok())
            .map(|hash| Fingerprint(*hash.as_bytes()))
            .ok_or_else(|| FingerprintError(text.to_owned()))
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// The signature of `goal`: its name, inputs, output and every record type they reach (`runtime/32` §2). It is what
/// a parent's `Call.goal_signature` and keys hold for this goal, never its artifact (D-11, R-ART-02).
pub fn signature(program: &Program, goal: GoalId) -> Result<Fingerprint, Diagnostic> {
    let goal = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    signature_of(program, goal).ok_or_else(Diagnostic::internal_error)
}

/// The `contract_key` of `goal`: its normalized source, its children's signatures, the language version and the IR and
/// builtins compatibility units (`runtime/32` §2, D-26, D-85). It decides lock staleness and seeds generated inputs.
pub fn contract_key(program: &Program, goal: GoalId) -> Result<Fingerprint, Diagnostic> {
    let goal = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    Contract { program, goal }
        .document()
        .and_then(|doc| Fingerprint::of(&doc).ok())
        .ok_or_else(Diagnostic::internal_error)
}

/// What a synthesis run adds to a `contract_key` to make its `synthesis_key` (`runtime/32` §2). None of it decides
/// whether an artifact is valid, only whether a stored one can be reused (R-ART-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Synthesis<'a> {
    /// The provider's input version: the `prompt_version`, or an external backend's `request_version`.
    pub input_version: &'a str,
    /// The compiler's version; only its `MAJOR.MINOR` enters the key (R-ART-04).
    pub compiler_version: &'a str,
    /// The provider id.
    pub provider: &'a str,
    /// The model id: Ollama's `<model>@<digest>`, or an external backend's `backend_version` (`compiler/22` §3).
    pub model: &'a str,
}

/// The `synthesis_key` of a goal with `contract_key`, synthesized as `synthesis` says: the artifact-store lookup key
/// and replay fixture name (`runtime/32` §2).
pub fn synthesis_key(contract_key: Fingerprint, synthesis: &Synthesis<'_>) -> Result<Fingerprint, Diagnostic> {
    let doc = json!({
        "contract_key": contract_key,
        "input_version": synthesis.input_version,
        "compiler": major_minor(synthesis.compiler_version),
        "provider": synthesis.provider,
        "model": synthesis.model,
    });
    Fingerprint::of(&doc).map_err(|_| Diagnostic::internal_error())
}

/// The `execution_id` of `artifact` run with children whose execution ids are `children`, in binding order: the
/// exact tree identity (`runtime/32` §2). Unlike the keys, it changes when a child's artifact does (R-ART-02).
pub fn execution_id(artifact: Fingerprint, children: &[Fingerprint]) -> Fingerprint {
    // Canonical by construction: the keys are in order, and a fingerprint's text needs no escaping.
    let children: Vec<String> = children.iter().map(|c| format!("\"{c}\"")).collect();
    let doc = format!(r#"{{"artifact":"{artifact}","children":[{}]}}"#, children.join(","));
    Fingerprint::of_bytes(doc.as_bytes())
}

/// The compatibility unit of a `MAJOR.MINOR` version (D-85): `MAJOR`, or `MAJOR.MINOR` while `MAJOR` is 0, as Cargo
/// reads versions. Versions of one unit read each other's artifacts; a new unit makes them stale.
pub fn compatibility(version: &str) -> &str {
    match version.split_once('.') {
        Some(("0", _)) => major_minor(version),
        Some((major, _)) => major,
        None => version,
    }
}

/// `MAJOR.MINOR` of a `MAJOR.MINOR.PATCH` version, so a patch release changes no key (R-ART-04).
fn major_minor(version: &str) -> &str {
    let Some((major, rest)) = version.split_once('.') else {
        return version;
    };
    let minor = rest.split(['.', '-', '+']).next().unwrap_or(rest);
    version.get(..major.len() + 1 + minor.len()).unwrap_or(version)
}

fn signature_of(program: &Program, goal: &hir::Goal) -> Option<Fingerprint> {
    let inputs = goal
        .params
        .iter()
        .map(|p| Some(json!([p.name, ir_type(program, &p.ty)?])))
        .collect::<Option<Vec<_>>>()?;
    let doc = json!({
        "goal": goal.name,
        "inputs": inputs,
        "output": ir_type(program, &goal.output)?,
        "types": reachable(program, goal.params.iter().map(|p| &p.ty).chain([&goal.output]))?,
    });
    Fingerprint::of(&doc).ok()
}

/// Every record type reachable from `roots`, through fields too, as IR (`compiler/21` R-IR-01).
pub(crate) fn reachable<'p>(
    program: &'p Program,
    roots: impl Iterator<Item = &'p HirType>,
) -> Option<BTreeMap<&'p str, RecordType>> {
    let mut pending = Vec::new();
    for ty in roots {
        records_in(ty, &mut pending);
    }
    let mut types = BTreeMap::new();
    while let Some(id) = pending.pop() {
        let record = program.record(id)?;
        if types.contains_key(record.name.as_str()) {
            continue;
        }
        let mut fields = Vec::new();
        for field in &record.fields {
            records_in(&field.ty, &mut pending);
            fields.push((field.name.clone(), ir_type(program, &field.ty)?));
        }
        types.insert(record.name.as_str(), RecordType { fields });
    }
    Some(types)
}

fn records_in(ty: &HirType, out: &mut Vec<TypeId>) {
    match ty {
        HirType::Optional(inner) | HirType::List(inner) => records_in(inner, out),
        HirType::Record(id) => out.push(*id),
        _ => {}
    }
}

/// The normalized source of one goal. `None` anywhere below means HIR that [`velme_sema::analyze`] doesn't return.
struct Contract<'p> {
    program: &'p Program,
    goal: &'p hir::Goal,
}

impl<'p> Contract<'p> {
    fn document(&self) -> Option<Json> {
        let goal = self.goal;
        let calls = goal
            .bindings
            .iter()
            .map(|b| {
                let callee = self.program.goals.get(b.callee.0)?;
                Some(json!({
                    "binding": b.name,
                    "goal": callee.name,
                    "goal_signature": signature_of(self.program, callee)?,
                    "args": self.exprs(&b.args)?,
                }))
            })
            .collect::<Option<Vec<_>>>()?;
        let examples = goal
            .examples
            .iter()
            .map(|e| Some(json!({"args": self.exprs(&e.args)?, "expected": self.expr(&e.expected)?})))
            .collect::<Option<Vec<_>>>()?;
        let budget = goal.budget;
        Some(json!({
            "signature": signature_of(self.program, goal)?,
            "plan": goal.plan,
            "calls": calls,
            "checks": self.exprs(&goal.checks)?,
            "examples": examples,
            // The effective limits, so a change to a system cap reaches every goal that doesn't lower it.
            "budget": {
                "max_fuel": budget.max_fuel,
                "max_memory": budget.max_memory,
                "max_goal_calls": budget.max_goal_calls,
                "max_call_depth": budget.max_call_depth,
                "max_list_size": MAX_LIST_SIZE,
                "max_output_bytes": MAX_OUTPUT_BYTES,
            },
            "language_version": self.program.language_version,
            "ir_compatibility": compatibility(IR_VERSION),
            "builtins_compatibility": compatibility(BUILTINS_VERSION),
        }))
    }

    fn exprs(&self, exprs: &[Expr]) -> Option<Vec<Json>> {
        exprs.iter().map(|e| self.expr(e)).collect()
    }

    fn expr(&self, expr: &Expr) -> Option<Json> {
        let goal = self.goal;
        Some(match &expr.kind {
            ExprKind::Number { text } => json!({"kind": "number", "value": number(text)?}),
            ExprKind::Text { value } => json!({"kind": "text", "value": value}),
            ExprKind::Bool { value } => json!({"kind": "bool", "value": value}),
            ExprKind::Nothing => json!({"kind": "nothing"}),
            ExprKind::Input { index } => json!({"kind": "input", "name": goal.params.get(*index)?.name}),
            ExprKind::Binding { index } => json!({"kind": "binding", "name": goal.bindings.get(*index)?.name}),
            ExprKind::Result => json!({"kind": "result"}),
            ExprKind::Var { depth } => json!({"kind": "var", "depth": depth}),
            ExprKind::Builtin { name, args } => json!({"kind": "builtin", "name": name, "args": self.exprs(args)?}),
            ExprKind::Record { record, fields } => {
                let record = self.program.record(*record)?;
                let fields = record
                    .fields
                    .iter()
                    .zip(fields)
                    .map(|(def, value)| Some(json!([def.name, self.expr(value)?])))
                    .collect::<Option<Vec<_>>>()?;
                json!({"kind": "record", "type": record.name, "fields": fields})
            }
            ExprKind::List { items } => json!({"kind": "list", "items": self.exprs(items)?}),
            ExprKind::Field { base, field } => json!({
                "kind": "field",
                "base": self.expr(base)?,
                "field": self.field_name(&base.ty, *field)?,
            }),
            ExprKind::Project { base, field } => {
                let element = match strip_optional(&base.ty) {
                    HirType::List(element) => element,
                    _ => return None,
                };
                json!({"kind": "project", "base": self.expr(base)?, "field": self.field_name(element, *field)?})
            }
            ExprKind::Unary { op, operand } => {
                json!({"kind": "unary", "op": op.as_str(), "operand": self.expr(operand)?})
            }
            ExprKind::Binary { op, lhs, rhs } => {
                json!({"kind": "binary", "op": op.as_str(), "lhs": self.expr(lhs)?, "rhs": self.expr(rhs)?})
            }
            ExprKind::IsEmpty { operand, negated } => {
                json!({"kind": "is_empty", "operand": self.expr(operand)?, "negated": negated})
            }
            ExprKind::If { condition, then } => {
                json!({"kind": "if", "condition": self.expr(condition)?, "then": self.expr(then)?})
            }
            // The variable's name is left out: it is `var` at this depth wherever it is used.
            ExprKind::Quantified {
                quantifier,
                collection,
                body,
                ..
            } => json!({
                "kind": "quantified",
                "quantifier": quantifier.as_str(),
                "collection": self.expr(collection)?,
                "body": self.expr(body)?,
            }),
        })
    }

    /// The name of field `index` of the record type `ty` holds.
    fn field_name(&self, ty: &HirType, index: usize) -> Option<&'p str> {
        let HirType::Record(id) = strip_optional(ty) else {
            return None;
        };
        Some(self.program.record(*id)?.fields.get(index)?.name.as_str())
    }
}

/// `T` of `T?`, or `ty` itself.
fn strip_optional(ty: &HirType) -> &HirType {
    match ty {
        HirType::Optional(inner) => inner,
        other => other,
    }
}
