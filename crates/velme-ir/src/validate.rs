//! The IR validator (`compiler/21` §6): the trust boundary every IR crosses before a back end runs it (INV-1).
//!
//! Stage 1 is parsing ([`from_json_str`]). Stages 2–4 and 7 are one walk that tags each finding with its stage; stage
//! 6 compares `calls`. Only the findings of the earliest failing stage are reported, which is what running the stages
//! in order and stopping at the first failure reports. Stage 5 has nothing to check in v0.1: no node can name a host
//! function or capability, and unknown fields already fail the schema (D-63).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use velme_builtins::{BUILTINS_VERSION, Builtin, CATALOG, Shape};
use velme_diagnostics::{Code, Diagnostic, Span, did_you_mean};
use velme_sema::hir::{self, GoalId, Keyword, Program, Type as HirType, TypeId, assignable, join, signature_output};

use crate::json::pointer;
use crate::limits::{MAX_COLLECTION_NESTING, MAX_DEPTH, MAX_IR_BYTES, MAX_LIST_ITEMS, MAX_NODES, MAX_TEXT_BYTES};
use crate::lower::ir_type;
use crate::mapping::{DecodeProblem, decode_value};
use crate::node::{
    BinaryOperator, Call, CallNode, Goal, Lambda, LiteralValue, Node, RecordType, ReduceLambda, Type, UnaryOperator,
};
use crate::{IR_VERSION, ParseError, from_json_str, to_canonical_string};

/// Where the IR being validated comes from, which decides what its `calls` may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A synthesis reply: it must omit `calls`, and the request's are joined in (R-IR-02, `compiler/20` R-CMP-08).
    Candidate,
    /// A whole goal from the compiler or the artifact store: its `calls` must equal the request's (R-IR-16).
    Complete,
}

/// What the IR must implement: one goal of a checked program (`compiler/21` §6).
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The checked program.
    pub program: &'a Program,
    /// The goal the IR implements.
    pub goal: GoalId,
    /// The compiler's call section for the goal, with its children's signatures (R-IR-09); empty for a leaf goal.
    pub calls: &'a [CallNode],
    /// Where the IR comes from.
    pub origin: Origin,
}

/// IR that passed every validation stage (`compiler/21` §6): the only form a back end accepts (INV-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidIr(Goal);

impl ValidIr {
    /// The validated goal, with the request's `calls` joined in for a candidate.
    pub fn goal(&self) -> &Goal {
        &self.0
    }

    /// The arguments of the goal's `index`th call, one expression per parameter of the callee, for a back end to
    /// evaluate before it runs the callee; empty if there is no such call. They were validated with the rest of the
    /// goal (R-IR-09) and equal the compiler's (R-IR-16).
    pub fn call_args(&self, index: usize) -> Vec<Trusted<'_>> {
        let Some(CallNode::Call(call)) = self.0.calls.get(index) else {
            return Vec::new();
        };
        call.args
            .iter()
            .map(|node| Trusted {
                node,
                types: &self.0.types,
            })
            .collect()
    }

    /// The goal's body, for a back end to evaluate.
    pub fn body(&self) -> Trusted<'_> {
        Trusted {
            node: &self.0.body,
            types: &self.0.types,
        }
    }

    /// The validated goal, by value.
    pub fn into_goal(self) -> Goal {
        self.0
    }
}

/// IR a back end may evaluate (INV-1): the body of a [`ValidIr`], or a [`TrustedExpr`], with the record types it is
/// typed over. Only this crate makes one, so a back end can't be handed an expression the validator hasn't seen.
#[derive(Debug, Clone, Copy)]
pub struct Trusted<'ir> {
    node: &'ir Node,
    types: &'ir BTreeMap<String, RecordType>,
}

impl<'ir> Trusted<'ir> {
    /// The expression.
    pub fn node(self) -> &'ir Node {
        self.node
    }

    /// The record types in scope, by name.
    pub fn types(self) -> &'ir BTreeMap<String, RecordType> {
        self.types
    }
}

/// A check item or example lowered to IR (`language/13` R-CHK-11) that passed stages 2–4 in its goal's check scope
/// ([`CheckScope`]): the one other form a back end evaluates (INV-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedExpr {
    node: Node,
    types: Arc<BTreeMap<String, RecordType>>,
}

impl TrustedExpr {
    /// The expression.
    pub fn node(&self) -> &Node {
        &self.node
    }

    /// The expression, for a back end to evaluate.
    pub fn trusted(&self) -> Trusted<'_> {
        Trusted {
            node: &self.node,
            types: &self.types,
        }
    }
}

/// What a goal's checks and examples may name (`language/13` R-CHK-02): its inputs, its call bindings and `result`,
/// and every record type of the program, since an example may build a record of any type. It exists only for the
/// check lowering of R-CHK-11, whose input is checked source: stage 7's limits bound what an LLM writes, and a valid
/// check can pass them — a 200-operator chain nests past 128, five nested quantifiers pass the collection nesting of
/// 4, a list literal can hold more than 1 000 items and a text more than 64 KiB — so they aren't applied. Its depth is
/// bounded by the source line instead (`language/10` D-71, `runtime/30` R-RUN-25). Never pass it IR from elsewhere.
#[derive(Debug, Clone)]
pub struct CheckScope<'p> {
    program: &'p Program,
    goal: &'p hir::Goal,
    types: Arc<BTreeMap<String, RecordType>>,
}

impl<'p> CheckScope<'p> {
    /// The check scope of `goal`, a goal of the checked `program`.
    pub fn new(program: &'p Program, goal: &'p hir::Goal) -> Result<Self, Diagnostic> {
        let types = program
            .types
            .iter()
            .map(|record| {
                let fields = record
                    .fields
                    .iter()
                    .map(|f| Some((f.name.clone(), ir_type(program, &f.ty)?)))
                    .collect::<Option<Vec<_>>>()?;
                Some((record.name.clone(), RecordType { fields }))
            })
            .collect::<Option<BTreeMap<_, _>>>()
            .ok_or_else(Diagnostic::internal_error)?;
        Ok(CheckScope {
            program,
            goal,
            types: Arc::new(types),
        })
    }

    /// `node`, an expression lowered from the goal's checks or examples, validated at stages 2–4 (`compiler/21` §6)
    /// in this scope, with a type assignable to `expected`: no `call` node or reused name either. A finding is a
    /// lowering bug: the source was checked.
    pub fn validate(&self, node: Node, expected: &HirType) -> Result<TrustedExpr, Vec<Diagnostic>> {
        let target = self.goal;
        let holder = Goal {
            ir_version: IR_VERSION.to_owned(),
            builtins_version: BUILTINS_VERSION.to_owned(),
            goal: target.name.clone(),
            types: BTreeMap::new(),
            inputs: Vec::new(),
            output: Type::Nothing {},
            calls: Vec::new(),
            body: node,
        };
        let findings = {
            let mut v = Validator::new(self.program, target, &holder, &[], Origin::Complete, Vec::new());
            v.compiler_names = true;
            let result = Keyword::Result.as_str();
            // A wired goal's `result` binding is its output.
            for binding in target.bindings.iter().filter(|b| b.name != result) {
                v.scope.push((&binding.name, binding.ty.clone()));
            }
            v.scope.push((result, target.output.clone()));
            let ty = v.expr(&holder.body, 1, 0);
            if !assignable(&ty, expected) {
                let rule = format!("it is {}, where {} is needed", v.name(&ty), v.name(expected));
                v.find(Stage::Types, "types-1", rule);
            }
            v.findings
        };
        let mut diags: Vec<Diagnostic> = findings
            .into_iter()
            .filter(|f| f.stage != Stage::Resources)
            .map(|f| f.diagnostic(target.span))
            .collect();
        if !diags.is_empty() {
            velme_diagnostics::sort(&mut diags);
            return Err(diags);
        }
        Ok(TrustedExpr {
            node: holder.body,
            types: Arc::clone(&self.types),
        })
    }
}

/// Validates the IR document `text` against `request` (`compiler/21` §6). On failure, returns every diagnostic of the
/// first failing stage: `VL0401` for stage 1, `VL0402` for the others, each naming its JSON path (R-IR-19). Total and
/// deterministic on any input (R-IR-18).
pub fn validate(text: &str, request: &Request<'_>) -> Result<ValidIr, Vec<Diagnostic>> {
    validate_detailed(text, request).map_err(|found| found.into_iter().map(|f| f.diagnostic).collect())
}

/// What a finding is about, as data: what a caller may say about the rule without quoting the document (`compiler/22`
/// R-SYNTH-49). It can hold a name the document chose, so it is for choosing words, never for showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Subject {
    /// Nothing more than the rule.
    #[default]
    None,
    /// The input or local name that isn't there (`names-7`, `names-8`).
    Name(String),
    /// The record that has no such field (`names-9`, `names-10`).
    Record(String),
    /// The built-in name that isn't there (`names-11`).
    Builtin(String),
    /// `add`, `sub`, `mul` or `div` given a Text operand (`types-15`).
    TextArithmetic,
}

/// One finding of [`validate_detailed`]: the diagnostic, and which rule it is, without any of what the document said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    /// The diagnostic, as [`validate`] reports it.
    pub diagnostic: Diagnostic,
    /// The stage: `schema`, `structure`, `names`, `types`, `callgraph` or `resources`.
    pub stage: &'static str,
    /// The rule's stable id, such as `types-7`: the same rule has the same id whatever the document names.
    pub rule: &'static str,
    /// The JSON Pointer of the fault, empty for the whole document or when the parser gave none.
    pub path: String,
    /// What the finding is about, for choosing words.
    pub subject: Subject,
}

/// [`validate`], with each finding's stage, rule id and path beside its diagnostic (`compiler/22` R-SYNTH-31, D-95).
pub fn validate_detailed(text: &str, request: &Request<'_>) -> Result<ValidIr, Vec<Invalid>> {
    let Some(target) = request.program.goals.get(request.goal.0) else {
        return Err(vec![Invalid::internal()]);
    };
    // A §7 limit, checked at stage 2 in the stage order, but before parsing so an oversized document costs nothing
    // to reject (R-IR-18); an oversized document that is also malformed is therefore `VL0402`, not `VL0401`.
    if text.len() > MAX_IR_BYTES {
        let rule = format!("it is {} bytes long; at most {MAX_IR_BYTES} are allowed", text.len());
        return Err(vec![
            Finding::new(Stage::Structure, "structure-1", String::new(), rule).invalid(target.span),
        ]);
    }
    let mut goal: Goal = from_json_str(text).map_err(|e| vec![schema_error(e, target.span)])?;
    let mut findings = Vec::new();
    if request.origin == Origin::Candidate {
        if !goal.calls.is_empty() {
            findings.push(Finding::new(
                Stage::Structure,
                "structure-2",
                "/calls".to_owned(),
                "it lists calls to other goals, which only the goal's `call` block can make".to_owned(),
            ));
        }
        goal.calls = request.calls.to_vec();
    }
    let findings = {
        let mut v = Validator::new(request.program, target, &goal, request.calls, request.origin, findings);
        v.envelope();
        v.walk();
        if request.origin == Origin::Complete {
            v.call_graph(request.calls);
        }
        v.findings
    };
    let Some(first) = findings.iter().map(|f| f.stage).min() else {
        return canonical_size(&goal, target.span).map(|()| ValidIr(goal));
    };
    let mut found: Vec<Invalid> = findings
        .into_iter()
        .filter(|f| f.stage == first)
        .map(|f| f.invalid(target.span))
        .collect();
    // `sort_by_key` is stable, so ties keep the walk's order as `velme_diagnostics::sort` does.
    found.sort_by_key(|f| (f.diagnostic.span.start, f.diagnostic.code));
    Err(found)
}

/// The §7 size limit on the canonical form of a goal that passed every other stage, which is what a store keeps: plain
/// decimals can be far longer than the numbers written (`1e27` is 28 digits), and a candidate's joined-in `calls` add
/// to it (R-IR-18, `runtime/32` R-ART-10). Checked last, so the walk over a document that fails a stage, however deep,
/// is the parser's alone.
fn canonical_size(goal: &Goal, span: Span) -> Result<(), Vec<Invalid>> {
    let length = to_canonical_string(goal).map_or(0, |text| text.len());
    if length <= MAX_IR_BYTES {
        return Ok(());
    }
    let rule = format!("its canonical form is {length} bytes long; at most {MAX_IR_BYTES} are allowed");
    Err(vec![
        Finding::new(Stage::Structure, "structure-3", String::new(), rule).invalid(span),
    ])
}

/// Stage 1 (R-IR-11): the document is not JSON of the schema's shape.
fn schema_error(error: ParseError, span: Span) -> Invalid {
    let diag = Diagnostic::new(
        Code::IRSchemaInvalid,
        span,
        "The generated program wasn't in the right shape.",
    );
    let (diagnostic, path) = match error {
        ParseError::DuplicateKey { pointer } => (
            diag.with_note(format!("at `{pointer}`: this key appears twice")),
            pointer,
        ),
        ParseError::ReservedKey { pointer } => {
            (diag.with_note(format!("at `{pointer}`: this key is reserved")), pointer)
        }
        ParseError::TooDeep { pointer, limit } => (
            diag.with_note(format!("at `{pointer}`: it nests deeper than {limit} levels")),
            pointer,
        ),
        // serde_json reports a line and column rather than a path.
        ParseError::Json(error) => (diag.with_note(error.to_string()), String::new()),
    };
    Invalid {
        diagnostic,
        stage: "schema",
        rule: "schema-1",
        path,
        subject: Subject::None,
    }
}

impl Invalid {
    fn internal() -> Invalid {
        Invalid {
            diagnostic: Diagnostic::internal_error(),
            stage: "internal",
            rule: "internal-1",
            path: String::new(),
            subject: Subject::None,
        }
    }
}

/// The validation stages after the schema, in order (`compiler/21` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    /// Stage 2 (R-IR-12).
    Structure,
    /// Stage 3 (R-IR-13).
    Names,
    /// Stage 4 (R-IR-14).
    Types,
    /// Stage 6 (R-IR-16).
    CallGraph,
    /// Stage 7 (R-IR-17).
    Resources,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Structure => "structure",
            Stage::Names => "names",
            Stage::Types => "types",
            Stage::CallGraph => "callgraph",
            Stage::Resources => "resources",
        }
    }
}

/// One broken rule, at a JSON path.
struct Finding {
    stage: Stage,
    /// The rule's stable id: the stage's name and a number, which never depends on what the document says.
    id: &'static str,
    path: String,
    rule: String,
    help: Option<String>,
    subject: Subject,
}

impl Finding {
    fn new(stage: Stage, id: &'static str, path: String, rule: String) -> Self {
        Finding {
            stage,
            id,
            path,
            rule,
            help: None,
            subject: Subject::None,
        }
    }

    fn invalid(self, span: Span) -> Invalid {
        Invalid {
            stage: self.stage.name(),
            rule: self.id,
            path: self.path.clone(),
            subject: self.subject.clone(),
            diagnostic: self.diagnostic(span),
        }
    }

    fn diagnostic(self, span: Span) -> Diagnostic {
        let at = if self.path.is_empty() {
            "at the whole program".to_owned()
        } else {
            format!("at `{}`", self.path)
        };
        let diag = Diagnostic::new(
            Code::IRInvalid,
            span,
            format!("The generated program broke a rule: {}.", self.rule),
        )
        .with_note(at);
        match self.help {
            Some(help) => diag.with_help(help),
            None => diag,
        }
    }
}

/// The walk over one goal. Types are the HIR's, so IR is typed by the same rules as checks (`language/11`); a
/// [`HirType::Error`] stands for a part that already has a finding and matches anything, so it doesn't cascade.
struct Validator<'a> {
    program: &'a Program,
    target: &'a hir::Goal,
    goal: &'a Goal,
    /// The compiler's call section for the goal: what its `types` may reach, whatever the IR's own `calls` say.
    calls: &'a [CallNode],
    origin: Origin,
    /// Set while walking what the compiler wrote — a candidate's joined-in `calls`, or a lowered check — whose names
    /// resolve against the program and the goal's signature, not against declarations in the IR.
    compiler_names: bool,
    /// The goal's inputs, with their types resolved.
    inputs: Vec<(&'a str, HirType)>,
    /// Call bindings, `let` names and lambda parameters in scope, innermost last.
    scope: Vec<(&'a str, HirType)>,
    /// The JSON path of the part being checked.
    path: Vec<String>,
    findings: Vec<Finding>,
    nodes: usize,
    too_deep: bool,
    too_nested: bool,
}

impl<'a> Validator<'a> {
    fn new(
        program: &'a Program,
        target: &'a hir::Goal,
        goal: &'a Goal,
        calls: &'a [CallNode],
        origin: Origin,
        findings: Vec<Finding>,
    ) -> Self {
        Validator {
            program,
            target,
            goal,
            calls,
            origin,
            compiler_names: false,
            inputs: Vec::new(),
            scope: Vec::new(),
            path: Vec::new(),
            findings,
            nodes: 0,
            too_deep: false,
            too_nested: false,
        }
    }

    fn find(&mut self, stage: Stage, id: &'static str, rule: String) {
        self.help(stage, id, rule, None);
    }

    fn help(&mut self, stage: Stage, id: &'static str, rule: String, help: Option<String>) {
        self.findings.push(Finding {
            stage,
            id,
            path: pointer(&self.path),
            rule,
            help,
            subject: Subject::None,
        });
    }

    /// Says what the finding just recorded is about.
    fn about(&mut self, subject: Subject) {
        if let Some(found) = self.findings.last_mut() {
            found.subject = subject;
        }
    }

    /// A finding at `segments` below the current path.
    fn find_at(&mut self, segments: &[&str], stage: Stage, id: &'static str, rule: String, help: Option<String>) {
        let mark = self.path.len();
        self.path.extend(segments.iter().map(|s| (*s).to_owned()));
        self.help(stage, id, rule, help);
        self.path.truncate(mark);
    }

    fn name(&self, ty: &HirType) -> String {
        self.program.type_name(ty)
    }

    /// Stage 2: the document's versions (R-IR-12, R-IR-22, D-63).
    fn envelope(&mut self) {
        let goal = self.goal;
        self.version("ir_version", "format version", &goal.ir_version, IR_VERSION);
        self.version(
            "builtins_version",
            "built-ins version",
            &goal.builtins_version,
            BUILTINS_VERSION,
        );
    }

    /// A candidate's version must equal the request's, which in v0.1 is always this build's own (D-63). A complete goal,
    /// such as a stored artifact, may be of an older minor of the same compatibility unit, which from 1.0 is the major:
    /// minor bumps are then additive (D-85; R-IR-22 for `ir_version`, `language/14` R-BLT-09 for `builtins_version`).
    /// A newer minor is named as such.
    fn version(&mut self, segment: &str, what: &str, found: &str, own: &str) {
        if found == own || (self.origin == Origin::Complete && readable(found, own)) {
            return;
        }
        let newer = matches!(
            (major_minor(found), major_minor(own)),
            (Some((major, minor)), Some((own_major, own_minor))) if major == own_major && minor > own_minor
        );
        let rule = if newer {
            format!("it uses {what} {found}, newer than the {own} this version of Velme reads")
        } else {
            format!("it uses {what} {found}, but {own} was asked for")
        };
        self.find_at(&[segment], Stage::Structure, "structure-4", rule, None);
    }

    /// Stages 2, 3, 4 and 7 over the whole goal.
    fn walk(&mut self) {
        let goal = self.goal;
        let target = self.target;
        if goal.goal != target.name {
            self.find_at(
                &["goal"],
                Stage::Names,
                "names-1",
                format!(
                    "it is for the goal `{}`, but `{}` was asked for",
                    goal.goal, target.name
                ),
                None,
            );
        }
        self.record_types();
        self.signature();
        self.calls();
        self.path.push("body".to_owned());
        let body = self.expr(&goal.body, 1, 0);
        self.path.pop();
        if !assignable(&body, &target.output) {
            let rule = format!(
                "its result is {}, but the goal `{}` gives {}",
                self.name(&body),
                target.name,
                self.name(&target.output)
            );
            self.find_at(&["body"], Stage::Types, "types-2", rule, None);
        }
        if self.nodes > MAX_NODES {
            self.path.clear();
            self.find(
                Stage::Resources,
                "resources-1",
                format!("it has {} expressions; at most {MAX_NODES} are allowed", self.nodes),
            );
        }
    }

    /// `types` equal the program's record types of the same names (R-IR-01, R-IR-14), and are only those the goal's
    /// inputs, output and calls reach (D-84).
    fn record_types(&mut self) {
        let goal = self.goal;
        let reached = self.reached();
        self.path.push("types".to_owned());
        for (name, record) in &goal.types {
            self.path.push(name.clone());
            match self.program.types.iter().enumerate().find(|(_, r)| &r.name == name) {
                None => {
                    let help = did_you_mean(name, self.program.types.iter().map(|r| r.name.as_str()));
                    self.help(
                        Stage::Names,
                        "names-2",
                        format!("it describes a record type `{name}` that the program doesn't have"),
                        help,
                    );
                }
                Some((id, _)) if !reached.contains(&TypeId(id)) => self.find(
                    Stage::Names,
                    "names-3",
                    format!("it describes a record type `{name}` that the goal's inputs, output and calls don't use"),
                ),
                Some((_, declared)) => {
                    let fields = self.fields(record);
                    let same = fields.len() == declared.fields.len()
                        && fields
                            .iter()
                            .zip(&declared.fields)
                            .all(|((n, t), f)| *n == f.name && *t == f.ty);
                    if !same {
                        let written: Vec<String> = declared
                            .fields
                            .iter()
                            .map(|f| format!("{}: {}", f.name, self.name(&f.ty)))
                            .collect();
                        self.help(
                            Stage::Types,
                            "types-3",
                            format!("its record type `{name}` differs from the program's"),
                            Some(format!("the program's is `{name}({})`", written.join(", "))),
                        );
                    }
                }
            }
            self.path.pop();
        }
        self.path.pop();
    }

    /// The record types the goal's inputs and output and its calls' goals reach, through fields too (D-84). The calls
    /// are the compiler's, so IR that drops or changes one is reported at stage 6, not as a type it doesn't use.
    fn reached(&self) -> Vec<TypeId> {
        let target = self.target;
        let callees = self
            .calls
            .iter()
            .filter_map(|CallNode::Call(call)| self.program.goals.iter().find(|g| g.name == call.goal));
        let mut pending = Vec::new();
        for goal in [target].into_iter().chain(callees) {
            for ty in goal.params.iter().map(|p| &p.ty).chain([&goal.output]) {
                records_in(ty, &mut pending);
            }
        }
        let mut reached = Vec::new();
        while let Some(id) = pending.pop() {
            if reached.contains(&id) {
                continue;
            }
            reached.push(id);
            for field in self.program.record(id).into_iter().flat_map(|r| &r.fields) {
                records_in(&field.ty, &mut pending);
            }
        }
        reached
    }

    /// A `types` entry's fields, with their types resolved.
    fn fields(&mut self, record: &'a RecordType) -> Vec<(&'a str, HirType)> {
        self.path.push("fields".to_owned());
        let fields = record
            .fields
            .iter()
            .enumerate()
            .map(|(i, (name, ty))| {
                self.path.push(i.to_string());
                let ty = self.resolve_at("1", ty);
                self.path.pop();
                (name.as_str(), ty)
            })
            .collect();
        self.path.pop();
        fields
    }

    /// `inputs` and `output` equal the goal's signature (R-IR-14).
    fn signature(&mut self) {
        let goal = self.goal;
        let target = self.target;
        self.path.push("inputs".to_owned());
        for (i, (name, ty)) in goal.inputs.iter().enumerate() {
            self.path.push(i.to_string());
            self.path.push("1".to_owned());
            let ty = self.resolve(ty);
            self.path.pop();
            self.path.pop();
            self.inputs.push((name, ty));
        }
        let same = self.inputs.len() == target.params.len()
            && self
                .inputs
                .iter()
                .zip(&target.params)
                .all(|((n, t), p)| *n == p.name && *t == p.ty);
        if !same {
            let listed = |items: Vec<String>| items.join(", ");
            let found = listed(
                self.inputs
                    .iter()
                    .map(|(n, t)| format!("{n}: {}", self.name(t)))
                    .collect(),
            );
            let wanted = listed(
                target
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, self.name(&p.ty)))
                    .collect(),
            );
            self.find(
                Stage::Types,
                "types-4",
                format!(
                    "its inputs are ({found}), but the goal `{}` takes ({wanted})",
                    target.name
                ),
            );
        }
        self.path.pop();
        self.path.push("output".to_owned());
        let output = self.resolve(&goal.output);
        if output != target.output {
            let rule = format!(
                "it gives {}, but the goal `{}` gives {}",
                self.name(&output),
                target.name,
                self.name(&target.output)
            );
            self.find(Stage::Types, "types-5", rule);
        }
        self.path.pop();
    }

    /// The call section: each call's goal, arguments and binding (R-IR-09).
    fn calls(&mut self) {
        let goal = self.goal;
        self.compiler_names = self.origin == Origin::Candidate;
        self.path.push("calls".to_owned());
        for (i, CallNode::Call(call)) in goal.calls.iter().enumerate() {
            self.path.push(i.to_string());
            self.count(1);
            let callee = self.program.goals.iter().find(|g| g.name == call.goal);
            if let Some(callee) = callee {
                self.listed(call, callee);
            } else {
                let help = did_you_mean(&call.goal, self.program.goals.iter().map(|g| g.name.as_str()));
                let rule = format!("it calls a goal `{}` that the program doesn't have", call.goal);
                self.find_at(&["goal"], Stage::Names, "names-4", rule, help);
            }
            self.path.push("args".to_owned());
            let args: Vec<HirType> = call
                .args
                .iter()
                .enumerate()
                .map(|(j, arg)| self.child(&j.to_string(), arg, 2, 0))
                .collect();
            self.path.pop();
            if let Some(callee) = callee {
                if args.len() != callee.params.len() {
                    let rule = format!(
                        "the call `{}` gives `{}` {} inputs, but it takes {}",
                        call.binding,
                        callee.name,
                        args.len(),
                        callee.params.len()
                    );
                    self.find_at(&["args"], Stage::Types, "types-6", rule, None);
                }
                for (j, (arg, param)) in args.iter().zip(&callee.params).enumerate() {
                    if !assignable(arg, &param.ty) {
                        let rule = format!(
                            "the call `{}` gives `{}` {} for `{}`, which needs {}",
                            call.binding,
                            callee.name,
                            self.name(arg),
                            param.name,
                            self.name(&param.ty)
                        );
                        self.find_at(&["args", &j.to_string()], Stage::Types, "types-7", rule, None);
                    }
                }
            }
            let ty = callee.map_or(HirType::Error, |c| c.output.clone());
            self.path.push("binding".to_owned());
            self.bind(&call.binding, ty);
            self.path.pop();
            self.path.pop();
        }
        self.path.pop();
        self.compiler_names = false;
    }

    /// R-IR-01: `types` lists every record type in the signature of a called goal, as it does for the goal's own.
    fn listed(&mut self, call: &Call, callee: &hir::Goal) {
        let mut records = Vec::new();
        for ty in callee.params.iter().map(|p| &p.ty).chain([&callee.output]) {
            records_in(ty, &mut records);
        }
        let goal = self.goal;
        for id in records {
            let Some(record) = self.program.record(id) else {
                continue;
            };
            if !goal.types.contains_key(&record.name) {
                let rule = format!(
                    "it doesn't describe the record type `{}`, which the call `{}` uses",
                    record.name, call.binding
                );
                let mark = std::mem::take(&mut self.path);
                self.find_at(&["types"], Stage::Names, "names-5", rule, None);
                self.path = mark;
            }
        }
    }

    /// Stage 6 (R-IR-16): `calls` equal the compiler's, call for call.
    fn call_graph(&mut self, expected: &[CallNode]) {
        let found = &self.goal.calls;
        self.path.clear();
        self.path.push("calls".to_owned());
        for i in 0..found.len().max(expected.len()) {
            let (rule, at) = match (found.get(i), expected.get(i)) {
                (Some(a), Some(b)) if a == b => continue,
                (Some(CallNode::Call(a)), Some(CallNode::Call(b))) if a.binding == b.binding => (
                    format!(
                        "its call `{}` differs from the one in the goal's `call` block",
                        a.binding
                    ),
                    Some(i),
                ),
                (Some(CallNode::Call(a)), Some(CallNode::Call(b))) => (
                    format!(
                        "it has the call `{}` where the goal's `call` block has `{}`",
                        a.binding, b.binding
                    ),
                    Some(i),
                ),
                (Some(CallNode::Call(a)), None) => (
                    format!("it has a call `{}` that the goal's `call` block doesn't", a.binding),
                    Some(i),
                ),
                (None, Some(CallNode::Call(b))) => (
                    format!("it leaves out the call `{}` of the goal's `call` block", b.binding),
                    None,
                ),
                (None, None) => continue,
            };
            match at {
                Some(i) => self.find_at(&[&i.to_string()], Stage::CallGraph, "callgraph-1", rule, None),
                None => self.find(Stage::CallGraph, "callgraph-2", rule),
            }
        }
        self.path.pop();
    }

    /// `ty` with its record names resolved against `types` and the program (R-IR-13).
    fn resolve(&mut self, ty: &Type) -> HirType {
        match ty {
            Type::Number {} => HirType::Number,
            Type::Text {} => HirType::Text,
            Type::Boolean {} => HirType::Boolean,
            Type::Nothing {} => HirType::Nothing,
            Type::Optional { of } => optional(self.resolve_at("of", of)),
            Type::List { of } => HirType::List(Box::new(self.resolve_at("of", of))),
            Type::Record { name } => {
                self.path.push("name".to_owned());
                let ty = self.record_named(name);
                self.path.pop();
                ty
            }
        }
    }

    fn resolve_at(&mut self, segment: &str, ty: &Type) -> HirType {
        self.path.push(segment.to_owned());
        let ty = self.resolve(ty);
        self.path.pop();
        ty
    }

    /// The record type `name`: listed in `types` (R-IR-01) and declared by the program.
    fn record_named(&mut self, name: &str) -> HirType {
        let goal = self.goal;
        if !self.compiler_names && !goal.types.contains_key(name) {
            let help = did_you_mean(name, goal.types.keys().map(String::as_str));
            self.help(
                Stage::Names,
                "names-6",
                format!("there's no record type called `{name}`"),
                help,
            );
            return HirType::Error;
        }
        // One missing from the program has its finding at `/types`.
        self.program
            .types
            .iter()
            .position(|r| r.name == name)
            .map_or(HirType::Error, |i| HirType::Record(TypeId(i)))
    }

    /// Binds a local; no local may shadow another (R-IR-12).
    fn bind(&mut self, name: &'a str, ty: HirType) {
        if self.scope.iter().any(|(n, _)| *n == name) {
            self.help(
                Stage::Structure,
                "structure-5",
                format!("the name `{name}` is already in use here"),
                Some("give it a name of its own".to_owned()),
            );
        }
        self.scope.push((name, ty));
    }

    /// Counts `n` more nodes toward the §7 limit.
    fn count(&mut self, n: usize) {
        self.nodes = self.nodes.saturating_add(n);
    }

    fn child(&mut self, segment: &str, node: &'a Node, depth: usize, nesting: usize) -> HirType {
        self.path.push(segment.to_owned());
        let ty = self.expr(node, depth, nesting);
        self.path.pop();
        ty
    }

    /// The type of `node` at nesting `depth` (the root is 1), inside `nesting` collection nodes (R-IR-14, R-IR-17).
    /// Each kind is its own method, so a debug build's stack frame per level stays small (R-IR-18).
    fn expr(&mut self, node: &'a Node, depth: usize, nesting: usize) -> HirType {
        self.count(1);
        if depth > MAX_DEPTH && !self.too_deep {
            self.too_deep = true;
            self.find(
                Stage::Resources,
                "resources-2",
                format!("it nests expressions more than {MAX_DEPTH} deep"),
            );
        }
        let inner = depth + 1;
        match node {
            Node::Literal { ty, value } => self.literal(ty, value),
            Node::Input { name } => self.input(name),
            Node::Local { name } => self.local(name),
            Node::Record { ty, fields } => self.record(ty, fields, inner, nesting),
            Node::List { of, items } => self.list(of, items, inner, nesting),
            Node::FieldGet { of, field } => self.field(of, field, inner, nesting),
            Node::BinaryOp { op, left, right } => self.binary(*op, left, right, inner, nesting),
            Node::UnaryOp { op, arg } => self.unary(*op, arg, inner, nesting),
            Node::Let { bind, body } => self.let_in(bind, body, inner, nesting),
            Node::Condition { cond, then, otherwise } => self.condition(cond, then, otherwise, inner, nesting),
            Node::Narrow { of, default } => self.narrow(of, default, inner, nesting),
            Node::Map { list, func, items } => {
                let (element, nesting) = self.list_of(node, list, inner, nesting);
                let result = self.lambda("fn", func, element, inner, nesting);
                items.set(matches!(result, HirType::Optional(_)));
                HirType::List(Box::new(result))
            }
            Node::Filter { list, func } => {
                let (element, nesting) = self.list_of(node, list, inner, nesting);
                self.predicate(node, func, element.clone(), inner, nesting);
                HirType::List(Box::new(element))
            }
            Node::Find { list, func } => {
                let (element, nesting) = self.list_of(node, list, inner, nesting);
                self.predicate(node, func, element.clone(), inner, nesting);
                optional(element)
            }
            Node::All { list, func } | Node::Any { list, func } => {
                let (element, nesting) = self.list_of(node, list, inner, nesting);
                self.predicate(node, func, element, inner, nesting);
                HirType::Boolean
            }
            Node::Reduce { list, init, func } => self.reduce(node, list, init, func, inner, nesting),
            Node::Sort { list, key, .. } => self.sort(node, list, key, inner, nesting),
            Node::Builtin { name, args } => self.builtin(name, args, inner, nesting),
            Node::Call(call) => {
                self.find(
                    Stage::Structure,
                    "structure-6",
                    format!(
                        "it calls the goal `{}` from inside an expression; only the goal's `call` block calls goals",
                        call.goal
                    ),
                );
                HirType::Error
            }
        }
    }

    /// `value` decodes as `ty` under the D-23 mapping, and its arrays and texts fit §7. The decoded value is kept in
    /// the node, so a back end never decodes it again (D-83).
    fn literal(&mut self, ty: &Type, value: &LiteralValue) -> HirType {
        let ty = self.resolve_at("type", ty);
        self.path.push("value".to_owned());
        match decode_value(value.json(), &ty, self.program) {
            Ok(decoded) => value.set_decoded(decoded),
            // `literal_sizes` reports long arrays against the tighter §7 limit.
            Err(error) if matches!(error.problem, DecodeProblem::TooManyItems { .. }) => {}
            Err(error) => {
                let help = match &error.problem {
                    DecodeProblem::UnknownField { help, .. } => help.clone(),
                    _ => None,
                };
                let segments: Vec<&str> = error.path.iter().map(String::as_str).collect();
                self.find_at(&segments, Stage::Types, "types-8", error.problem.to_string(), help);
            }
        }
        self.literal_sizes(value.json());
        self.path.pop();
        ty
    }

    /// §7 limits on a literal's arrays and texts.
    fn literal_sizes(&mut self, value: &Value) {
        match value {
            Value::String(text) if text.len() > MAX_TEXT_BYTES => self.find(
                Stage::Resources,
                "resources-3",
                format!(
                    "this text is {} bytes long; at most {MAX_TEXT_BYTES} are allowed",
                    text.len()
                ),
            ),
            Value::Array(items) => {
                if items.len() > MAX_LIST_ITEMS {
                    self.find(
                        Stage::Resources,
                        "resources-4",
                        format!(
                            "this list has {} items; at most {MAX_LIST_ITEMS} are allowed",
                            items.len()
                        ),
                    );
                }
                for (i, item) in items.iter().enumerate() {
                    self.path.push(i.to_string());
                    self.literal_sizes(item);
                    self.path.pop();
                }
            }
            Value::Object(members) => {
                for (key, item) in members {
                    self.path.push(key.clone());
                    self.literal_sizes(item);
                    self.path.pop();
                }
            }
            _ => {}
        }
    }

    fn input(&mut self, name: &str) -> HirType {
        let target = self.target;
        let found = if self.compiler_names {
            target.params.iter().find(|p| p.name == name).map(|p| &p.ty)
        } else {
            self.inputs.iter().find(|(n, _)| *n == name).map(|(_, ty)| ty)
        };
        if let Some(ty) = found {
            return ty.clone();
        }
        let help = if self.compiler_names {
            did_you_mean(name, target.params.iter().map(|p| p.name.as_str()))
        } else {
            did_you_mean(name, self.inputs.iter().map(|(n, _)| *n))
        };
        self.find_at(
            &["name"],
            Stage::Names,
            "names-7",
            format!("there's no input called `{name}`"),
            help,
        );
        self.about(Subject::Name(name.to_owned()));
        HirType::Error
    }

    fn local(&mut self, name: &str) -> HirType {
        if let Some((_, ty)) = self.scope.iter().rev().find(|(n, _)| *n == name) {
            return ty.clone();
        }
        let help = did_you_mean(name, self.scope.iter().map(|(n, _)| *n));
        self.find_at(
            &["name"],
            Stage::Names,
            "names-8",
            format!("there's no name `{name}` here"),
            help,
        );
        self.about(Subject::Name(name.to_owned()));
        HirType::Error
    }

    /// Exactly the declared fields, each assignable (R-TYP-20).
    fn record(&mut self, name: &str, fields: &'a BTreeMap<String, Node>, depth: usize, nesting: usize) -> HirType {
        self.path.push("type".to_owned());
        let ty = self.record_named(name);
        self.path.pop();
        let declared = match &ty {
            HirType::Record(id) => self.program.record(*id),
            _ => None,
        };
        self.path.push("fields".to_owned());
        for (field, value) in fields {
            let found = self.child(field, value, depth, nesting);
            let Some(record) = declared else { continue };
            match record.field(field) {
                None => {
                    let help = did_you_mean(field, record.fields.iter().map(|f| f.name.as_str()));
                    let rule = format!("`{}` has no field `{field}`", record.name);
                    self.find_at(&[field], Stage::Names, "names-9", rule, help);
                    self.about(Subject::Record(record.name.clone()));
                }
                Some(f) if !assignable(&found, &f.ty) => {
                    let rule = format!(
                        "the field `{field}` of `{}` holds {}, not {}",
                        record.name,
                        self.name(&f.ty),
                        self.name(&found)
                    );
                    self.find_at(&[field], Stage::Types, "types-9", rule, None);
                }
                Some(_) => {}
            }
        }
        if let Some(record) = declared {
            let missing: Vec<String> = record
                .fields
                .iter()
                .filter(|f| !fields.contains_key(&f.name))
                .map(|f| format!("`{}`", f.name))
                .collect();
            if !missing.is_empty() {
                self.find(
                    Stage::Types,
                    "types-10",
                    format!("a `{}` needs the fields {}", record.name, missing.join(", ")),
                );
            }
        }
        self.path.pop();
        ty
    }

    /// Each item assignable to `of` (R-TYP-13: lists are invariant, so the list's type is `List<of>`).
    fn list(&mut self, of: &Type, items: &'a [Node], depth: usize, nesting: usize) -> HirType {
        let of = self.resolve_at("of", of);
        if items.len() > MAX_LIST_ITEMS {
            self.find(
                Stage::Resources,
                "resources-5",
                format!(
                    "this list has {} items; at most {MAX_LIST_ITEMS} are allowed",
                    items.len()
                ),
            );
        }
        self.path.push("items".to_owned());
        for (i, item) in items.iter().enumerate() {
            let found = self.child(&i.to_string(), item, depth, nesting);
            if !assignable(&found, &of) {
                let rule = format!(
                    "this item is {}, but the list holds {}",
                    self.name(&found),
                    self.name(&of)
                );
                self.find_at(&[&i.to_string()], Stage::Types, "types-11", rule, None);
            }
        }
        self.path.pop();
        HirType::List(Box::new(of))
    }

    /// A field of a record that is not optional (R-IR-05).
    fn field(&mut self, of: &'a Node, field: &str, depth: usize, nesting: usize) -> HirType {
        let base = self.child("of", of, depth, nesting);
        match &base {
            HirType::Record(id) => {
                let Some(record) = self.program.record(*id) else {
                    return HirType::Error;
                };
                if let Some(f) = record.field(field) {
                    return f.ty.clone();
                }
                let help = did_you_mean(field, record.fields.iter().map(|f| f.name.as_str()));
                let rule = format!("`{}` has no field `{field}`", record.name);
                self.find_at(&["field"], Stage::Names, "names-10", rule, help);
                self.about(Subject::Record(record.name.clone()));
            }
            HirType::Optional(_) => {
                let rule = format!(
                    "this {} may be nothing, so its fields can't be read yet",
                    self.name(&base)
                );
                self.help(
                    Stage::Types,
                    "types-12",
                    rule,
                    Some("narrow it first with `unwrap_or`, or `if` and `is_empty`".to_owned()),
                );
            }
            HirType::Error => {}
            other => {
                let rule = format!("{} has no fields", self.name(other));
                self.find(Stage::Types, "types-13", rule);
            }
        }
        HirType::Error
    }

    /// `compiler/21` §3 `binary`: equality needs one side assignable to the other (R-TYP-20, D-61).
    fn binary(&mut self, op: BinaryOperator, left: &'a Node, right: &'a Node, depth: usize, nesting: usize) -> HirType {
        let l = self.child("left", left, depth, nesting);
        let r = self.child("right", right, depth, nesting);
        let (operand, result) = match op {
            BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul | BinaryOperator::Div => {
                (HirType::Number, HirType::Number)
            }
            BinaryOperator::Lt | BinaryOperator::Le | BinaryOperator::Gt | BinaryOperator::Ge => {
                (HirType::Number, HirType::Boolean)
            }
            BinaryOperator::And | BinaryOperator::Or => (HirType::Boolean, HirType::Boolean),
            BinaryOperator::Eq | BinaryOperator::Ne => {
                if !assignable(&l, &r) && !assignable(&r, &l) {
                    let rule = format!("`{}` can't compare {} with {}", wire(&op), self.name(&l), self.name(&r));
                    self.find(Stage::Types, "types-14", rule);
                }
                return HirType::Boolean;
            }
        };
        if !is(&l, &operand) || !is(&r, &operand) {
            let rule = format!(
                "`{}` needs two {}s, not {} and {}",
                wire(&op),
                self.name(&operand),
                self.name(&l),
                self.name(&r)
            );
            self.find(Stage::Types, "types-15", rule);
            let arithmetic = matches!(
                op,
                BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul | BinaryOperator::Div
            );
            if arithmetic && (matches!(l, HirType::Text) || matches!(r, HirType::Text)) {
                self.about(Subject::TextArithmetic);
            }
        }
        result
    }

    /// `compiler/21` §3 `unary`: `is_empty` takes an optional, a list or text (D-60).
    fn unary(&mut self, op: UnaryOperator, arg: &'a Node, depth: usize, nesting: usize) -> HirType {
        let found = self.child("arg", arg, depth, nesting);
        let (fits, wanted, result) = match op {
            UnaryOperator::Neg => (is(&found, &HirType::Number), "a Number", HirType::Number),
            UnaryOperator::Not => (is(&found, &HirType::Boolean), "a Boolean", HirType::Boolean),
            UnaryOperator::IsEmpty => (
                matches!(
                    found,
                    HirType::Optional(_) | HirType::Nothing | HirType::List(_) | HirType::Text | HirType::Error
                ),
                "a list, a text or a value that may be nothing",
                HirType::Boolean,
            ),
        };
        if !fits {
            let rule = format!("`{}` needs {wanted}, not {}", wire(&op), self.name(&found));
            self.find(Stage::Types, "types-16", rule);
        }
        result
    }

    /// Sequential bindings, each visible to the later ones and the body (R-IR-12: no shadowing).
    fn let_in(&mut self, bind: &'a [(String, Node)], body: &'a Node, depth: usize, nesting: usize) -> HirType {
        let mark = self.scope.len();
        self.path.push("bind".to_owned());
        for (i, (name, value)) in bind.iter().enumerate() {
            self.path.push(i.to_string());
            let ty = self.child("1", value, depth, nesting);
            self.path.push("0".to_owned());
            self.bind(name, ty);
            self.path.pop();
            self.path.pop();
        }
        self.path.pop();
        let ty = self.child("body", body, depth, nesting);
        self.scope.truncate(mark);
        ty
    }

    /// A `Boolean` condition; the branches join (`T` and `Nothing` give `T?`, R-TYP-26).
    fn condition(
        &mut self,
        cond: &'a Node,
        then: &'a Node,
        otherwise: &'a Node,
        depth: usize,
        nesting: usize,
    ) -> HirType {
        let c = self.child("cond", cond, depth, nesting);
        if !is(&c, &HirType::Boolean) {
            let rule = format!("the condition of `if` must be a Boolean, not {}", self.name(&c));
            self.find_at(&["cond"], Stage::Types, "types-17", rule, None);
        }
        let a = self.child("then", then, depth, nesting);
        let b = self.child("else", otherwise, depth, nesting);
        join(&a, &b).unwrap_or_else(|| {
            let rule = format!(
                "the branches of `if` give {} and {}, which don't mix",
                self.name(&a),
                self.name(&b)
            );
            self.find(Stage::Types, "types-18", rule);
            HirType::Error
        })
    }

    /// `of: T?` and `default: T` give `T` (R-IR-05).
    fn narrow(&mut self, of: &'a Node, default: &'a Node, depth: usize, nesting: usize) -> HirType {
        let found = self.child("of", of, depth, nesting);
        let fallback = self.child("default", default, depth, nesting);
        let present = match found {
            HirType::Optional(inner) => *inner,
            HirType::Nothing => fallback.clone(),
            HirType::Error => HirType::Error,
            other => {
                let rule = format!(
                    "`unwrap_or` needs a value that may be nothing, not {}",
                    self.name(&other)
                );
                self.find_at(&["of"], Stage::Types, "types-19", rule, None);
                other
            }
        };
        if !assignable(&fallback, &present) {
            let rule = format!(
                "the default of `unwrap_or` is {}, but the value is {}",
                self.name(&fallback),
                self.name(&present)
            );
            self.find_at(&["default"], Stage::Types, "types-20", rule, None);
        }
        present
    }

    /// The element type of a collection node's `list`, and the collection nesting inside the node's lambda (R-IR-17).
    /// Only lambdas nest (D-79): an op in `list` position runs once, so a flat pipeline is linear in cost.
    fn list_of(&mut self, node: &Node, list: &'a Node, depth: usize, nesting: usize) -> (HirType, usize) {
        let inside = nesting + 1;
        if inside > MAX_COLLECTION_NESTING && !self.too_nested {
            self.too_nested = true;
            self.find(
                Stage::Resources,
                "resources-6",
                format!(
                    "it nests list operations more than {MAX_COLLECTION_NESTING} deep inside one another's functions"
                ),
            );
        }
        let found = self.child("list", list, depth, nesting);
        let element = match found {
            HirType::List(element) => *element,
            HirType::Error => HirType::Error,
            other => {
                let rule = format!("`{}` needs a list, not {}", node.kind(), self.name(&other));
                self.find_at(&["list"], Stage::Types, "types-21", rule, None);
                HirType::Error
            }
        };
        (element, inside)
    }

    /// The body of `func`, with its parameter bound to `element` (R-IR-04).
    fn lambda(&mut self, segment: &str, func: &'a Lambda, element: HirType, depth: usize, nesting: usize) -> HirType {
        self.path.push(segment.to_owned());
        let mark = self.scope.len();
        self.path.push("param".to_owned());
        self.bind(&func.param, element);
        self.path.pop();
        let ty = self.child("body", &func.body, depth, nesting);
        self.scope.truncate(mark);
        self.path.pop();
        ty
    }

    /// A `fn` whose body must be a `Boolean` (`filter`, `find`, `all`, `any`; D-58).
    fn predicate(&mut self, node: &Node, func: &'a Lambda, element: HirType, depth: usize, nesting: usize) {
        let ty = self.lambda("fn", func, element, depth, nesting);
        if !is(&ty, &HirType::Boolean) {
            let rule = format!(
                "the function of `{}` must give a Boolean, not {}",
                node.kind(),
                self.name(&ty)
            );
            self.find_at(&["fn", "body"], Stage::Types, "types-22", rule, None);
        }
    }

    /// `init: B` and a body of `B` give `B`.
    fn reduce(
        &mut self,
        node: &Node,
        list: &'a Node,
        init: &'a Node,
        func: &'a ReduceLambda,
        depth: usize,
        nesting: usize,
    ) -> HirType {
        let (element, nesting) = self.list_of(node, list, depth, nesting);
        let acc = self.child("init", init, depth, nesting);
        self.path.push("fn".to_owned());
        let mark = self.scope.len();
        self.path.push("acc".to_owned());
        self.bind(&func.acc, acc.clone());
        self.path.pop();
        self.path.push("param".to_owned());
        self.bind(&func.param, element);
        self.path.pop();
        let next = self.child("body", &func.body, depth, nesting);
        self.scope.truncate(mark);
        if !assignable(&next, &acc) {
            let rule = format!(
                "the function of `reduce` gives {}, but it starts from {}",
                self.name(&next),
                self.name(&acc)
            );
            self.find_at(&["body"], Stage::Types, "types-23", rule, None);
        }
        self.path.pop();
        acc
    }

    /// A `Number` key only (R-IR-07, D-59).
    fn sort(&mut self, node: &Node, list: &'a Node, key: &'a Lambda, depth: usize, nesting: usize) -> HirType {
        let (element, nesting) = self.list_of(node, list, depth, nesting);
        let ty = self.lambda("key", key, element.clone(), depth, nesting);
        if !is(&ty, &HirType::Number) {
            let (rule, help) = if ty == HirType::Text {
                (
                    "`sort_by` can't sort by text yet; its key must give a Number".to_owned(),
                    Some("sort by a number instead".to_owned()),
                )
            } else {
                (
                    format!("the key of `sort_by` must give a Number, not {}", self.name(&ty)),
                    None,
                )
            };
            self.find_at(&["key", "body"], Stage::Types, "types-24", rule, help);
        }
        HirType::List(Box::new(element))
    }

    /// A built-in of the catalog, matched against its signatures in order (`language/14`, D-63).
    fn builtin(&mut self, name: &str, args: &'a [Node], depth: usize, nesting: usize) -> HirType {
        self.path.push("args".to_owned());
        let found: Vec<HirType> = args
            .iter()
            .enumerate()
            .map(|(i, arg)| self.child(&i.to_string(), arg, depth, nesting))
            .collect();
        self.path.pop();
        let Some(builtin) = Builtin::find(name).filter(|b| b.in_ir) else {
            let help = did_you_mean(name, CATALOG.iter().filter(|b| b.in_ir).map(|b| b.name));
            self.find_at(
                &["name"],
                Stage::Names,
                "names-11",
                format!("there's no built-in called `{name}`"),
                help,
            );
            self.about(Subject::Builtin(name.to_owned()));
            return HirType::Error;
        };
        let takes_function = |shape: &Shape| matches!(shape, Shape::Lambda(..));
        if builtin.signatures.iter().any(|s| s.params.iter().any(takes_function)) {
            let rule = format!("`{name}` takes a function, so it is written as a `{name}` node, not a built-in call");
            self.find_at(&["name"], Stage::Types, "types-25", rule, None);
            return HirType::Error;
        }
        if let Some(ty) = builtin.signatures.iter().find_map(|s| signature_output(s, &found)) {
            return ty;
        }
        let rule = if builtin.signatures.iter().any(|s| s.params.len() == found.len()) {
            let listed: Vec<String> = found.iter().map(|t| self.name(t)).collect();
            format!("`{name}` can't take ({})", listed.join(", "))
        } else {
            let arities: Vec<String> = builtin.signatures.iter().map(|s| s.params.len().to_string()).collect();
            format!("`{name}` takes {} inputs, not {}", arities.join(" or "), found.len())
        };
        self.find(Stage::Types, "types-26", rule);
        HirType::Error
    }
}

/// The record types in `ty`, each once, in the order first met.
fn records_in(ty: &HirType, out: &mut Vec<TypeId>) {
    match ty {
        HirType::Optional(inner) | HirType::List(inner) => records_in(inner, out),
        HirType::Record(id) if !out.contains(id) => out.push(*id),
        _ => {}
    }
}

/// `ty` is `wanted`, or already has a finding.
fn is(ty: &HirType, wanted: &HirType) -> bool {
    *ty == HirType::Error || ty == wanted
}

/// `T?`, where `T??` is `T?` (§2.1).
fn optional(ty: HirType) -> HirType {
    match ty {
        HirType::Optional(_) | HirType::Error => ty,
        other => HirType::Optional(Box::new(other)),
    }
}

/// The wire spelling of an operator, from its serde mapping (CC-CONST-03).
fn wire<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Whether a validator of version `own` reads version `found`: the same compatibility unit — `MAJOR`, or
/// `MAJOR.MINOR` while `MAJOR` is 0 (D-85) — and a minor no newer (R-IR-22).
fn readable(found: &str, own: &str) -> bool {
    matches!(
        (major_minor(found), major_minor(own)),
        (Some((major, minor)), Some((own_major, own_minor)))
            if major == own_major && (minor == own_minor || (major > 0 && minor < own_minor))
    )
}

/// `MAJOR.MINOR` as numbers (R-IR-22).
fn major_minor(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    // One spelling per number, since the compatibility unit is compared as text (D-85): no leading zero.
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0')))
            .then(|| s.parse().ok())
            .flatten()
    };
    Some((number(major)?, number(minor)?))
}

#[cfg(test)]
mod tests {
    use super::readable;

    /// R-IR-22, D-85: a validator reads its own compatibility unit — `MAJOR`, or `MAJOR.MINOR` while `MAJOR` is 0 — at
    /// a minor no newer than its own.
    #[test]
    fn versions_are_read_within_their_compatibility_unit() {
        for (found, own, read) in [
            ("0.1", "0.1", true),
            ("0.0", "0.1", false),
            ("0.2", "0.1", false),
            ("1.0", "1.2", true),
            ("1.2", "1.2", true),
            ("1.3", "1.2", false),
            ("2.0", "1.2", false),
            ("1.0", "0.1", false),
            ("1", "1.0", false),
            ("0.01", "0.1", false),
            ("00.1", "0.1", false),
            ("01.0", "1.0", false),
            ("1.00", "1.0", false),
            ("1.0", "01.0", false),
        ] {
            assert_eq!(readable(found, own), read, "{found} by {own}");
        }
    }
}
