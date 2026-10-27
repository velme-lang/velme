//! Fingerprints (`runtime/32` §2, D-11, D-26): BLAKE3 over canonical JSON (D-21), written `b3:<hex>`.
//!
//! Each key hashes a small JSON document built here, never a Rust type's derived serialization, so a compiler refactor
//! cannot change a key (R-ART-04): operators are their surface spelling, and the only serialized types are the IR's own
//! (`Type`, `RecordType`), whose form is the versioned IR schema (`compiler/21` R-IR-20). The goal source enters as
//! normalized structure (R-ART-23, D-81).
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value as Json, json};
use velme_builtins::BUILTINS_VERSION;
use velme_builtins::limits::{MAX_LIST_SIZE, MAX_OUTPUT_BYTES};
use velme_diagnostics::Diagnostic;
use velme_sema::hir::{self, DomainId, DomainRule, Expr, ExprKind, GoalId, Program, Type as HirType, TypeId};

use crate::lower::{Tables, domain_entry, ir_type, number};
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
    Contract {
        program,
        goal: Some(goal),
    }
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

/// The signature document (R-ART-26): `goal`, `inputs`, `output` and `types` as before domains, and `domains` only
/// when the goal reaches one, so a domain-free goal keeps its key (R-ART-27).
fn signature_of(program: &Program, goal: &hir::Goal) -> Option<Fingerprint> {
    Fingerprint::of(&signature_doc(program, goal)?).ok()
}

/// The JSON document whose hash is [`signature`] (R-ART-26), for golden tests (AC-ART-16).
#[doc(hidden)]
pub fn signature_document(program: &Program, goal: GoalId) -> Option<Json> {
    signature_doc(program, program.goals.get(goal.0)?)
}

fn signature_doc(program: &Program, goal: &hir::Goal) -> Option<Json> {
    let inputs = goal
        .params
        .iter()
        .map(|p| Some(json!([p.name, sig_type(program, &p.ty)?])))
        .collect::<Option<Vec<_>>>()?;
    let mut reach = Reach::new(Reachability::Contract);
    for ty in goal.params.iter().map(|p| &p.ty).chain([&goal.output]) {
        reach.ty(program, ty)?;
    }
    let mut doc = json!({
        "goal": goal.name,
        "inputs": inputs,
        "output": sig_type(program, &goal.output)?,
        "types": reach.types(program)?,
    });
    if !reach.domains.is_empty()
        && let Some(map) = doc.as_object_mut()
    {
        map.insert("domains".to_owned(), Json::Array(reach.domain_nodes(program)?));
    }
    Some(doc)
}

/// The declaration hash of domain `id` (R-ART-26, for R-SYNTH-65): its normalized declaration and every declaration
/// it reaches through fields, its base and its condition's constructors, each once, without itself, in name order.
pub fn declaration_hash(program: &Program, id: DomainId) -> Result<Fingerprint, Diagnostic> {
    declaration_document(program, id)
        .and_then(|doc| Fingerprint::of(&doc).ok())
        .ok_or_else(Diagnostic::internal_error)
}

/// The JSON document whose hash is [`declaration_hash`] (R-ART-26), for golden tests.
#[doc(hidden)]
pub fn declaration_document(program: &Program, id: DomainId) -> Option<Json> {
    let mut reach = Reach::new(Reachability::Contract);
    reach.domain_parts(program, id)?;
    let mut dependencies: Vec<(&str, Json)> = reach
        .records
        .iter()
        .map(|r| {
            let record = program.record(*r)?;
            Some((record.name.as_str(), record_node(program, record)?))
        })
        .collect::<Option<_>>()?;
    // A constructor cycle can reach the root again; it is the `declaration`, never its own dependency.
    for d in reach.domains.iter().filter(|d| **d != id) {
        let domain = program.domain(*d)?;
        dependencies.push((domain.name.as_str(), domain_node(program, domain)?));
    }
    dependencies.sort_by(|a, b| a.0.cmp(b.0));
    let declaration = domain_node(program, program.domain(id)?)?;
    let dependencies: Vec<Json> = dependencies.into_iter().map(|(_, node)| node).collect();
    Some(json!({"declaration": declaration, "dependencies": dependencies}))
}

/// A type in a signature: exactly the IR's own serialization, a domain written `{"t": "Domain", "name": …}`
/// (R-ART-26), so domain-free signatures keep their D1 bytes (R-ART-27).
fn sig_type(program: &Program, ty: &HirType) -> Option<Json> {
    serde_json::to_value(ir_type(program, ty)?).ok()
}

/// `[[name, type], …]` of `fields`.
fn field_nodes(program: &Program, fields: &[hir::FieldDef]) -> Option<Json> {
    fields
        .iter()
        .map(|f| Some(json!([f.name, sig_type(program, &f.ty)?])))
        .collect::<Option<Vec<_>>>()
        .map(Json::Array)
}

/// A plain record's declaration node.
fn record_node(program: &Program, record: &hir::RecordType) -> Option<Json> {
    Some(json!({"name": record.name, "fields": field_nodes(program, &record.fields)?}))
}

/// A domain's full normalized declaration (R-ART-26): name, then `base` and `one_of` or `where`; a record domain has
/// its `fields` instead of a base.
fn domain_node(program: &Program, domain: &hir::DomainType) -> Option<Json> {
    let mut node = serde_json::Map::new();
    node.insert("name".to_owned(), json!(domain.name));
    match &domain.base {
        HirType::Record(id) => {
            node.insert("fields".to_owned(), field_nodes(program, &program.record(*id)?.fields)?);
        }
        base => {
            node.insert("base".to_owned(), sig_type(program, base)?);
        }
    }
    match &domain.rule {
        DomainRule::OneOf { members } => node.insert("one_of".to_owned(), json!(members)),
        DomainRule::Where { condition } => {
            let normal = Contract { program, goal: None }.expr(condition)?;
            node.insert("where".to_owned(), normal)
        }
    };
    Some(Json::Object(node))
}

/// The domain whose underlying record is `id`, if any (R-TYP-28).
fn record_domain(program: &Program, id: TypeId) -> Option<DomainId> {
    program
        .domains
        .iter()
        .position(|d| d.base == HirType::Record(id))
        .map(DomainId)
}

/// Which declarations a type reaches: the IR's tables or a signature's (`compiler/21` R-IR-24, `runtime/32` R-ART-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    /// The IR's `types` and `domains` (R-IR-01): records and domains through fields and domain bases, a record
    /// domain's underlying record included; domain conditions aren't shipped in IR, so their constructors reach nothing.
    Ir,
    /// A signature's and a declaration hash's (R-ART-26), and the prompt's Types (`compiler/22` R-SYNTH-60): also the
    /// records the constructors in domain conditions name, transitively; a record domain stands for its underlying
    /// record, which isn't listed again.
    Contract,
}

/// The declarations some types reach, by [`Reachability`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declarations {
    /// The record types.
    pub records: BTreeSet<TypeId>,
    /// The domain types.
    pub domains: BTreeSet<DomainId>,
}

/// The declarations `roots` reach under `reachability`: the one reachability every IR table, signature and prompt
/// uses. `None` for HIR [`velme_sema::analyze`] doesn't return.
pub fn declarations<'p>(
    program: &'p Program,
    roots: impl IntoIterator<Item = &'p HirType>,
    reachability: Reachability,
) -> Option<Declarations> {
    let mut reach = Reach::new(reachability);
    for ty in roots {
        reach.ty(program, ty)?;
    }
    Some(Declarations {
        records: reach.records,
        domains: reach.domains,
    })
}

/// The declarations a signature, a declaration or an IR goal reaches (R-ART-26, R-IR-01): records through fields, and
/// domains through fields and bases, transitively; for a contract also through the record constructors in their
/// conditions.
struct Reach {
    reachability: Reachability,
    records: BTreeSet<TypeId>,
    domains: BTreeSet<DomainId>,
}

impl Reach {
    fn new(reachability: Reachability) -> Self {
        Reach {
            reachability,
            records: BTreeSet::new(),
            domains: BTreeSet::new(),
        }
    }

    fn ty(&mut self, program: &Program, ty: &HirType) -> Option<()> {
        match ty {
            HirType::Optional(inner) | HirType::List(inner) => self.ty(program, inner)?,
            HirType::Domain(id, _) => self.domain(program, *id)?,
            HirType::Record(id) => match record_domain(program, *id) {
                Some(domain) => self.domain(program, domain)?,
                None => self.record(program, *id)?,
            },
            _ => {}
        }
        Some(())
    }

    fn record(&mut self, program: &Program, id: TypeId) -> Option<()> {
        if self.records.insert(id) {
            for field in &program.record(id)?.fields {
                self.ty(program, &field.ty)?;
            }
        }
        Some(())
    }

    fn domain(&mut self, program: &Program, id: DomainId) -> Option<()> {
        if self.domains.insert(id) {
            self.domain_parts(program, id)?;
        }
        Some(())
    }

    /// What domain `id` itself refers to.
    fn domain_parts(&mut self, program: &Program, id: DomainId) -> Option<()> {
        let domain = program.domain(id)?;
        if let HirType::Record(record) = &domain.base {
            if self.reachability == Reachability::Ir {
                self.records.insert(*record);
            }
            for field in &program.record(*record)?.fields {
                self.ty(program, &field.ty)?;
            }
        }
        if self.reachability == Reachability::Contract
            && let DomainRule::Where { condition } = &domain.rule
        {
            self.constructors(program, condition)?;
        }
        Some(())
    }

    fn constructors(&mut self, program: &Program, e: &Expr) -> Option<()> {
        if let ExprKind::Record { record, .. } = &e.kind {
            self.ty(program, &HirType::Record(*record))?;
        }
        for child in children(e) {
            self.constructors(program, child)?;
        }
        Some(())
    }

    /// The `types` member: each plain record as `{"fields": …}`, by name.
    fn types(&self, program: &Program) -> Option<BTreeMap<String, Json>> {
        self.records
            .iter()
            .map(|id| {
                let record = program.record(*id)?;
                Some((
                    record.name.clone(),
                    json!({"fields": field_nodes(program, &record.fields)?}),
                ))
            })
            .collect()
    }

    /// The `domains` member: each domain's node, in name order.
    fn domain_nodes(&self, program: &Program) -> Option<Vec<Json>> {
        let mut nodes = self
            .domains
            .iter()
            .map(|d| {
                let domain = program.domain(*d)?;
                Some((domain.name.as_str(), domain_node(program, domain)?))
            })
            .collect::<Option<Vec<_>>>()?;
        nodes.sort_by(|a, b| a.0.cmp(b.0));
        Some(nodes.into_iter().map(|(_, node)| node).collect())
    }
}

/// The direct subexpressions of `e`.
fn children(e: &Expr) -> Vec<&Expr> {
    match &e.kind {
        ExprKind::Builtin { args: items, .. } | ExprKind::Record { fields: items, .. } | ExprKind::List { items } => {
            items.iter().collect()
        }
        ExprKind::Field { base, .. } | ExprKind::Project { base, .. } => vec![base],
        ExprKind::Unary { operand, .. } | ExprKind::IsEmpty { operand, .. } | ExprKind::IsWhole { operand, .. } => {
            vec![operand]
        }
        ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
        ExprKind::If { condition, then } => vec![condition, then],
        ExprKind::Quantified { collection, body, .. } => vec![collection, body],
        ExprKind::Number { .. }
        | ExprKind::Text { .. }
        | ExprKind::Bool { .. }
        | ExprKind::Nothing
        | ExprKind::Input { .. }
        | ExprKind::Binding { .. }
        | ExprKind::Result
        | ExprKind::Value
        | ExprKind::Var { .. } => Vec::new(),
    }
}

/// The IR's `types` and `domains` of a goal whose inputs, output and calls' signatures are `roots`, by name
/// (`compiler/21` R-IR-01, R-IR-24).
pub(crate) fn tables<'p>(program: &'p Program, roots: impl IntoIterator<Item = &'p HirType>) -> Option<Tables> {
    let reached = declarations(program, roots, Reachability::Ir)?;
    let types = reached
        .records
        .iter()
        .map(|id| {
            let record = program.record(*id)?;
            let fields = record
                .fields
                .iter()
                .map(|f| Some((f.name.clone(), ir_type(program, &f.ty)?)))
                .collect::<Option<_>>()?;
            Some((record.name.clone(), RecordType { fields }))
        })
        .collect::<Option<_>>()?;
    let domains = reached
        .domains
        .iter()
        .map(|id| {
            let domain = program.domain(*id)?;
            Some((domain.name.clone(), domain_entry(program, domain)?))
        })
        .collect::<Option<_>>()?;
    Some((types, domains))
}

/// The normalized source of one goal, or of a domain condition with no goal. `None` anywhere below means HIR that
/// [`velme_sema::analyze`] doesn't return.
struct Contract<'p> {
    program: &'p Program,
    goal: Option<&'p hir::Goal>,
}

impl<'p> Contract<'p> {
    fn document(&self) -> Option<Json> {
        let goal = self.goal?;
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
            ExprKind::Input { index } => json!({"kind": "input", "name": goal?.params.get(*index)?.name}),
            ExprKind::Binding { index } => json!({"kind": "binding", "name": goal?.bindings.get(*index)?.name}),
            ExprKind::Result => json!({"kind": "result"}),
            ExprKind::Value => json!({"kind": "value"}),
            // Keys keep the surface `is whole` node (R-ART-23), never its `floor` lowering.
            ExprKind::IsWhole { operand, negated } => {
                json!({"kind": "is_whole", "operand": self.expr(operand)?, "negated": negated})
            }
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
                    HirType::List(element) => element.widened(),
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
        let HirType::Record(id) = strip_optional(ty).widened() else {
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
