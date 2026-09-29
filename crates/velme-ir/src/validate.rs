//! The IR validator (`compiler/21` §6): the trust boundary every IR crosses before a back end runs it (INV-1).
//!
//! Stage 1 is parsing ([`from_json_str`]). Stages 2–4 and 7 are one walk that tags each finding with its stage; stage
//! 6 compares `calls`. Only the findings of the earliest failing stage are reported, which is what running the stages
//! in order and stopping at the first failure reports. Stage 5 has nothing to check in v0.1: no node can name a host
//! function or capability, and unknown fields already fail the schema (D-63).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use velme_builtins::{BUILTINS_VERSION, Builtin, CATALOG, Shape};
use velme_diagnostics::{Code, Diagnostic, Span, did_you_mean};
use velme_sema::hir::{self, GoalId, Program, Type as HirType, TypeId, assignable, join, signature_output};

use crate::json::pointer;
use crate::limits::{MAX_COLLECTION_NESTING, MAX_DEPTH, MAX_IR_BYTES, MAX_LIST_ITEMS, MAX_NODES, MAX_TEXT_BYTES};
use crate::mapping::{DecodeProblem, decode_value};
use crate::node::{BinaryOperator, Call, CallNode, Goal, Lambda, Node, RecordType, ReduceLambda, Type, UnaryOperator};
use crate::{IR_VERSION, MAX_JSON_DEPTH, ParseError, from_json_str};

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

    /// The validated goal, by value.
    pub fn into_goal(self) -> Goal {
        self.0
    }
}

/// Validates the IR document `text` against `request` (`compiler/21` §6). On failure, returns every diagnostic of the
/// first failing stage: `VL0401` for stage 1, `VL0402` for the others, each naming its JSON path (R-IR-19). Total and
/// deterministic on any input (R-IR-18).
pub fn validate(text: &str, request: &Request<'_>) -> Result<ValidIr, Vec<Diagnostic>> {
    let Some(target) = request.program.goals.get(request.goal.0) else {
        return Err(vec![Diagnostic::internal_error()]);
    };
    // A §7 limit, checked at stage 2 in the stage order, but before parsing so an oversized document costs nothing
    // to reject (R-IR-18); an oversized document that is also malformed is therefore `VL0402`, not `VL0401`.
    if text.len() > MAX_IR_BYTES {
        let rule = format!("it is {} bytes long; at most {MAX_IR_BYTES} are allowed", text.len());
        return Err(vec![
            Finding::new(Stage::Structure, String::new(), rule).diagnostic(target.span),
        ]);
    }
    let mut goal: Goal = from_json_str(text).map_err(|e| vec![schema_error(e, target.span)])?;
    let mut findings = Vec::new();
    if request.origin == Origin::Candidate {
        if !goal.calls.is_empty() {
            findings.push(Finding::new(
                Stage::Structure,
                "/calls".to_owned(),
                "it lists calls to other goals, which only the goal's `call` block can make".to_owned(),
            ));
        }
        goal.calls = request.calls.to_vec();
    }
    let findings = {
        let mut v = Validator::new(request.program, target, &goal, request.origin, findings);
        v.envelope();
        v.walk();
        if request.origin == Origin::Complete {
            v.call_graph(request.calls);
        }
        v.findings
    };
    let Some(first) = findings.iter().map(|f| f.stage).min() else {
        return Ok(ValidIr(goal));
    };
    let mut diags: Vec<Diagnostic> = findings
        .into_iter()
        .filter(|f| f.stage == first)
        .map(|f| f.diagnostic(target.span))
        .collect();
    velme_diagnostics::sort(&mut diags);
    Err(diags)
}

/// Stage 1 (R-IR-11): the document is not JSON of the schema's shape.
fn schema_error(error: ParseError, span: Span) -> Diagnostic {
    let diag = Diagnostic::new(
        Code::IRSchemaInvalid,
        span,
        "The generated program wasn't in the right shape.",
    );
    match error {
        ParseError::DuplicateKey { pointer } => diag.with_note(format!("at `{pointer}`: this key appears twice")),
        ParseError::ReservedKey { pointer } => diag.with_note(format!("at `{pointer}`: this key is reserved")),
        ParseError::TooDeep { pointer } => {
            diag.with_note(format!("at `{pointer}`: it nests deeper than {MAX_JSON_DEPTH} levels"))
        }
        // serde_json reports a line and column rather than a path.
        ParseError::Json(error) => diag.with_note(error.to_string()),
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

/// One broken rule, at a JSON path.
struct Finding {
    stage: Stage,
    path: String,
    rule: String,
    help: Option<String>,
}

impl Finding {
    fn new(stage: Stage, path: String, rule: String) -> Self {
        Finding {
            stage,
            path,
            rule,
            help: None,
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
    origin: Origin,
    /// Set while walking a candidate's joined-in `calls`: they come from the compiler, so their names resolve against
    /// the program and the goal's signature, not against declarations the candidate wrote.
    compiler_calls: bool,
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
        origin: Origin,
        findings: Vec<Finding>,
    ) -> Self {
        Validator {
            program,
            target,
            goal,
            origin,
            compiler_calls: false,
            inputs: Vec::new(),
            scope: Vec::new(),
            path: Vec::new(),
            findings,
            nodes: 0,
            too_deep: false,
            too_nested: false,
        }
    }

    fn find(&mut self, stage: Stage, rule: String) {
        self.help(stage, rule, None);
    }

    fn help(&mut self, stage: Stage, rule: String, help: Option<String>) {
        self.findings.push(Finding {
            stage,
            path: pointer(&self.path),
            rule,
            help,
        });
    }

    /// A finding at `segments` below the current path.
    fn find_at(&mut self, segments: &[&str], stage: Stage, rule: String, help: Option<String>) {
        let mark = self.path.len();
        self.path.extend(segments.iter().map(|s| (*s).to_owned()));
        self.help(stage, rule, help);
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
    /// such as a stored artifact, may be of an older minor of the same major: minor bumps are additive (R-IR-22 for
    /// `ir_version`, `language/14` R-BLT-09 for `builtins_version`). A newer minor is named as such.
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
        self.find_at(&[segment], Stage::Structure, rule, None);
    }

    /// Stages 2, 3, 4 and 7 over the whole goal.
    fn walk(&mut self) {
        let goal = self.goal;
        let target = self.target;
        if goal.goal != target.name {
            self.find_at(
                &["goal"],
                Stage::Names,
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
            self.find_at(&["body"], Stage::Types, rule, None);
        }
        if self.nodes > MAX_NODES {
            self.path.clear();
            self.find(
                Stage::Resources,
                format!("it has {} expressions; at most {MAX_NODES} are allowed", self.nodes),
            );
        }
    }

    /// `types` equal the program's record types of the same names (R-IR-01, R-IR-14).
    fn record_types(&mut self) {
        let goal = self.goal;
        self.path.push("types".to_owned());
        for (name, record) in &goal.types {
            self.path.push(name.clone());
            match self.program.types.iter().find(|r| &r.name == name) {
                None => {
                    let help = did_you_mean(name, self.program.types.iter().map(|r| r.name.as_str()));
                    self.help(
                        Stage::Names,
                        format!("it describes a record type `{name}` that the program doesn't have"),
                        help,
                    );
                }
                Some(declared) => {
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
            self.find(Stage::Types, rule);
        }
        self.path.pop();
    }

    /// The call section: each call's goal, arguments and binding (R-IR-09).
    fn calls(&mut self) {
        let goal = self.goal;
        self.compiler_calls = self.origin == Origin::Candidate;
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
                self.find_at(&["goal"], Stage::Names, rule, help);
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
                    self.find_at(&["args"], Stage::Types, rule, None);
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
                        self.find_at(&["args", &j.to_string()], Stage::Types, rule, None);
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
        self.compiler_calls = false;
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
                self.find_at(&["types"], Stage::Names, rule, None);
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
                Some(i) => self.find_at(&[&i.to_string()], Stage::CallGraph, rule, None),
                None => self.find(Stage::CallGraph, rule),
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
        if !self.compiler_calls && !goal.types.contains_key(name) {
            let help = did_you_mean(name, goal.types.keys().map(String::as_str));
            self.help(Stage::Names, format!("there's no record type called `{name}`"), help);
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
            Node::Map { list, func } => {
                let (element, nesting) = self.list_of(node, list, inner, nesting);
                HirType::List(Box::new(self.lambda("fn", func, element, inner, nesting)))
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
                    format!(
                        "it calls the goal `{}` from inside an expression; only the goal's `call` block calls goals",
                        call.goal
                    ),
                );
                HirType::Error
            }
        }
    }

    /// `value` decodes as `ty` under the D-23 mapping, and its arrays and texts fit §7.
    fn literal(&mut self, ty: &Type, value: &Value) -> HirType {
        let ty = self.resolve_at("type", ty);
        self.path.push("value".to_owned());
        // `literal_sizes` reports long arrays against the tighter §7 limit.
        if let Err(error) = decode_value(value, &ty, self.program)
            && !matches!(error.problem, DecodeProblem::TooManyItems { .. })
        {
            let help = match &error.problem {
                DecodeProblem::UnknownField { help, .. } => help.clone(),
                _ => None,
            };
            let segments: Vec<&str> = error.path.iter().map(String::as_str).collect();
            self.find_at(&segments, Stage::Types, error.problem.to_string(), help);
        }
        self.literal_sizes(value);
        self.path.pop();
        ty
    }

    /// §7 limits on a literal's arrays and texts.
    fn literal_sizes(&mut self, value: &Value) {
        match value {
            Value::String(text) if text.len() > MAX_TEXT_BYTES => self.find(
                Stage::Resources,
                format!(
                    "this text is {} bytes long; at most {MAX_TEXT_BYTES} are allowed",
                    text.len()
                ),
            ),
            Value::Array(items) => {
                if items.len() > MAX_LIST_ITEMS {
                    self.find(
                        Stage::Resources,
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
        if self.compiler_calls {
            let param = self.target.params.iter().find(|p| p.name == name);
            return param.map_or(HirType::Error, |p| p.ty.clone());
        }
        if let Some((_, ty)) = self.inputs.iter().find(|(n, _)| *n == name) {
            return ty.clone();
        }
        let help = did_you_mean(name, self.inputs.iter().map(|(n, _)| *n));
        self.find_at(
            &["name"],
            Stage::Names,
            format!("there's no input called `{name}`"),
            help,
        );
        HirType::Error
    }

    fn local(&mut self, name: &str) -> HirType {
        if let Some((_, ty)) = self.scope.iter().rev().find(|(n, _)| *n == name) {
            return ty.clone();
        }
        let help = did_you_mean(name, self.scope.iter().map(|(n, _)| *n));
        self.find_at(&["name"], Stage::Names, format!("there's no name `{name}` here"), help);
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
                    self.find_at(&[field], Stage::Names, rule, help);
                }
                Some(f) if !assignable(&found, &f.ty) => {
                    let rule = format!(
                        "the field `{field}` of `{}` holds {}, not {}",
                        record.name,
                        self.name(&f.ty),
                        self.name(&found)
                    );
                    self.find_at(&[field], Stage::Types, rule, None);
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
                self.find_at(&[&i.to_string()], Stage::Types, rule, None);
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
                self.find_at(&["field"], Stage::Names, rule, help);
            }
            HirType::Optional(_) => {
                let rule = format!(
                    "this {} may be nothing, so its fields can't be read yet",
                    self.name(&base)
                );
                self.help(
                    Stage::Types,
                    rule,
                    Some("narrow it first with `unwrap_or`, or `if` and `is_empty`".to_owned()),
                );
            }
            HirType::Error => {}
            other => {
                let rule = format!("{} has no fields", self.name(other));
                self.find(Stage::Types, rule);
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
                    self.find(Stage::Types, rule);
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
            self.find(Stage::Types, rule);
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
            self.find(Stage::Types, rule);
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
            self.find_at(&["cond"], Stage::Types, rule, None);
        }
        let a = self.child("then", then, depth, nesting);
        let b = self.child("else", otherwise, depth, nesting);
        join(&a, &b).unwrap_or_else(|| {
            let rule = format!(
                "the branches of `if` give {} and {}, which don't mix",
                self.name(&a),
                self.name(&b)
            );
            self.find(Stage::Types, rule);
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
                self.find_at(&["of"], Stage::Types, rule, None);
                other
            }
        };
        if !assignable(&fallback, &present) {
            let rule = format!(
                "the default of `unwrap_or` is {}, but the value is {}",
                self.name(&fallback),
                self.name(&present)
            );
            self.find_at(&["default"], Stage::Types, rule, None);
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
                self.find_at(&["list"], Stage::Types, rule, None);
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
            self.find_at(&["fn", "body"], Stage::Types, rule, None);
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
            self.find_at(&["body"], Stage::Types, rule, None);
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
            self.find_at(&["key", "body"], Stage::Types, rule, help);
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
                format!("there's no built-in called `{name}`"),
                help,
            );
            return HirType::Error;
        };
        let takes_function = |shape: &Shape| matches!(shape, Shape::Lambda(..));
        if builtin.signatures.iter().any(|s| s.params.iter().any(takes_function)) {
            let rule = format!("`{name}` takes a function, so it is written as a `{name}` node, not a built-in call");
            self.find_at(&["name"], Stage::Types, rule, None);
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
        self.find(Stage::Types, rule);
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

/// Whether a validator of version `own` reads version `found`: the same major and a minor no newer (R-IR-22).
fn readable(found: &str, own: &str) -> bool {
    matches!(
        (major_minor(found), major_minor(own)),
        (Some((major, minor)), Some((own_major, own_minor))) if major == own_major && minor <= own_minor
    )
}

/// `MAJOR.MINOR` as numbers (R-IR-22).
fn major_minor(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    Some((number(major)?, number(minor)?))
}
