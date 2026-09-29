//! Loading a goal's locked artifact (`runtime/32` R-ART-10, R-ART-14): the artifact is trusted only once its bytes,
//! its manifest and its IR have all been checked against the lock and the current source (D-46).

use std::fmt;

use velme_builtins::BUILTINS_VERSION;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::{
    CallNode, Fingerprint, IR_VERSION, Origin, Request, Type, ValidIr, calls, contract_key, ir_type, signature,
    to_canonical_string, validate,
};
use velme_sema::hir::{self, GoalId, Program};

use crate::artifact::{Artifact, Manifest};
use crate::lock::{Entry, Lock};
use crate::store::{LoadError, Store};

/// A goal's locked artifact, checked as `velme run` needs (R-ART-10): its bytes hash to its address, its manifest
/// agrees with its lock entry and the current source, and its IR validates. It is not re-verified (D-46).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedGoal {
    /// The artifact's address.
    pub artifact: Fingerprint,
    /// How it was produced.
    pub manifest: Manifest,
    /// Its IR.
    pub ir: ValidIr,
}

/// Loads the locked artifact of `goal`, declared in the project file `file` (`runtime/32` R-ART-10). An entry whose
/// keys differ from the ones computed from `program` is stale, and the causes name what changed (R-ART-14).
pub fn load(program: &Program, goal: GoalId, file: &str, lock: &Lock, store: &Store) -> Result<LockedGoal, EntryError> {
    let target = program.goals.get(goal.0).ok_or(EntryError::Internal)?;
    let entry = lock
        .entry(file, &target.name)
        .ok_or_else(|| EntryError::Stale(vec![Cause::NotLocked]))?;
    let computed_signature = signature(program, goal).map_err(|_| EntryError::Internal)?;
    let computed_contract = contract_key(program, goal).map_err(|_| EntryError::Internal)?;
    if entry.contract_key != computed_contract || entry.signature != computed_signature {
        // What the source was built from is known only from the stored artifact, which is read here just to word
        // the causes; its claims decide nothing (D-46). Without it, the source changed is all that is known.
        let Ok(artifact) = store.get(entry.artifact) else {
            return Err(EntryError::Stale(vec![Cause::Source]));
        };
        let manifest = &artifact.manifest;
        // Another goal's artifact says nothing about how this goal changed, and one built from the current source
        // means it is the lock entry that disagrees with it.
        let causes = if manifest.goal != target.name
            || (manifest.signature == computed_signature && manifest.contract_key == computed_contract)
        {
            mismatches(manifest, entry)
        } else {
            let causes = changes(program, goal, target, &artifact).ok_or(EntryError::Internal)?;
            if causes.is_empty() { vec![Cause::Source] } else { causes }
        };
        return Err(EntryError::Stale(causes));
    }
    let Artifact { manifest, ir } = store.get(entry.artifact).map_err(EntryError::Load)?;
    let mismatches = mismatches(&manifest, entry);
    if !mismatches.is_empty() {
        return Err(EntryError::Stale(mismatches));
    }
    let text = to_canonical_string(&ir).map_err(|_| EntryError::Internal)?;
    let calls = calls(program, goal).map_err(|_| EntryError::Internal)?;
    let request = Request {
        program,
        goal,
        calls: &calls,
        origin: Origin::Complete,
    };
    let ir = validate(&text, &request).map_err(|diags| EntryError::Stale(vec![Cause::Invalid(diags)]))?;
    Ok(LockedGoal {
        artifact: entry.artifact,
        manifest,
        ir,
    })
}

/// The fields where `manifest` disagrees with the lock entry it is pinned by (D-46).
fn mismatches(manifest: &Manifest, entry: &Entry) -> Vec<Cause> {
    // Another goal's artifact differs in its keys too; that it is another goal's is the whole story.
    if manifest.goal != entry.name {
        return vec![Cause::Manifest { field: "goal" }];
    }
    [
        ("signature", manifest.signature == entry.signature),
        ("contract_key", manifest.contract_key == entry.contract_key),
    ]
    .into_iter()
    .filter(|(_, agrees)| !agrees)
    .map(|(field, _)| Cause::Manifest { field })
    .collect()
}

/// What changed between the source `artifact` was built from and the current one, as far as the artifact shows it
/// (R-ART-14); empty if only the parts it doesn't record changed. `None` for HIR `analyze` doesn't return.
fn changes(program: &Program, id: GoalId, goal: &hir::Goal, artifact: &Artifact) -> Option<Vec<Cause>> {
    let manifest = &artifact.manifest;
    let ir = &artifact.ir;
    let mut causes = Vec::new();
    let versions = [
        (
            Versioned::Language,
            manifest.language_version.as_str(),
            program.language_version.as_str(),
        ),
        (Versioned::Ir, major(&manifest.ir_version), major(IR_VERSION)),
        (
            Versioned::Builtins,
            major(&manifest.builtins_version),
            major(BUILTINS_VERSION),
        ),
    ];
    for (of, built, current) in versions {
        if built != current {
            causes.push(Cause::Version {
                of,
                built: built.to_owned(),
                now: current.to_owned(),
            });
        }
    }
    for (name, built) in &ir.types {
        let Some(record) = program.types.iter().find(|r| &r.name == name) else {
            causes.push(Cause::Record {
                name: name.clone(),
                change: RecordChange::Gone,
            });
            continue;
        };
        let now = record
            .fields
            .iter()
            .map(|f| Some((f.name.clone(), ir_type(program, &f.ty)?)))
            .collect::<Option<Vec<_>>>()?;
        causes.extend(
            record_changes(&built.fields, &now)
                .into_iter()
                .map(|change| Cause::Record {
                    name: name.clone(),
                    change,
                }),
        );
    }
    let inputs = goal
        .params
        .iter()
        .map(|p| Some((p.name.clone(), ir_type(program, &p.ty)?)))
        .collect::<Option<Vec<_>>>()?;
    if ir.inputs != inputs {
        causes.push(Cause::Inputs);
    }
    if ir.output != ir_type(program, &goal.output)? {
        causes.push(Cause::Output);
    }
    let now = calls(program, id).ok()?;
    let same_calls = ir.calls.len() == now.len()
        && ir
            .calls
            .iter()
            .zip(&now)
            .all(|(CallNode::Call(a), CallNode::Call(b))| a.binding == b.binding && a.goal == b.goal);
    if !same_calls {
        causes.push(Cause::Calls);
    } else {
        for (CallNode::Call(built), CallNode::Call(call)) in ir.calls.iter().zip(&now) {
            let child = Cause::Child {
                goal: call.goal.clone(),
            };
            // A child called twice changed once.
            if built.goal_signature != call.goal_signature && !causes.contains(&child) {
                causes.push(child);
            }
        }
        if ir
            .calls
            .iter()
            .zip(&now)
            .any(|(CallNode::Call(a), CallNode::Call(b))| a.args != b.args)
        {
            causes.push(Cause::Calls);
        }
    }
    Some(causes)
}

/// How a record type's fields changed, by name: added, removed or of another type, else reordered.
fn record_changes(built: &[(String, Type)], now: &[(String, Type)]) -> Vec<RecordChange> {
    let find = |fields: &[(String, Type)], name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone());
    let mut changes = Vec::new();
    for (name, ty) in now {
        match find(built, name) {
            None => changes.push(RecordChange::Added(name.clone())),
            Some(old) if &old != ty => changes.push(RecordChange::Retyped(name.clone())),
            Some(_) => {}
        }
    }
    for (name, _) in built {
        if find(now, name).is_none() {
            changes.push(RecordChange::Removed(name.clone()));
        }
    }
    if changes.is_empty() && built != now {
        changes.push(RecordChange::Reordered);
    }
    changes
}

/// `MAJOR` of a `MAJOR.MINOR` version.
fn major(version: &str) -> &str {
    version.split_once('.').map_or(version, |(major, _)| major)
}

/// Why a goal's lock entry can't be used (`runtime/32` R-ART-14).
#[derive(Debug)]
pub enum EntryError {
    /// The entry is missing or stale: `VL0702`.
    Stale(Vec<Cause>),
    /// Its artifact could not be read: `VL0701`, `VL0703` or `VL0901`, or `VL0702` if it is no artifact document.
    Load(LoadError),
    /// The program has no such goal, or HIR `analyze` doesn't return: `VL0607`.
    Internal,
}

impl EntryError {
    /// The diagnostic code.
    pub fn code(&self) -> Code {
        match self {
            Self::Stale(_) => Code::LockStale,
            Self::Load(error) => error.code(),
            Self::Internal => Code::InternalError,
        }
    }

    /// The failure of the goal `goal` declared at `span`, worded as in `reference/90`, with a note per cause.
    pub fn diagnostic(&self, goal: &str, span: Span) -> Diagnostic {
        match self {
            Self::Stale(causes) => causes.iter().fold(
                Diagnostic::new(
                    self.code(),
                    span,
                    format!("`{goal}` changed since it was last built — run `velme build`."),
                ),
                |diag, cause| diag.with_note(cause.to_string()),
            ),
            Self::Load(error) => error.diagnostic(goal, span),
            Self::Internal => Diagnostic::internal_error(),
        }
    }
}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale(causes) => {
                let causes: Vec<String> = causes.iter().map(ToString::to_string).collect();
                write!(f, "the lock entry is stale: {}", causes.join("; "))
            }
            Self::Load(error) => write!(f, "{error}"),
            Self::Internal => write!(f, "the goal's keys could not be computed"),
        }
    }
}

impl std::error::Error for EntryError {}

/// Why a lock entry is stale, as the CLI names it (R-ART-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The goal has no entry.
    NotLocked,
    /// The artifact was built for another language, IR major or builtins major version.
    Version {
        /// Which version.
        of: Versioned,
        /// The one it was built for.
        built: String,
        /// The current one.
        now: String,
    },
    /// A record type the goal's IR uses changed.
    Record {
        /// The type.
        name: String,
        /// How.
        change: RecordChange,
    },
    /// The goal's inputs changed.
    Inputs,
    /// The goal's output type changed.
    Output,
    /// A goal it calls changed its signature (D-11).
    Child {
        /// The child goal.
        goal: String,
    },
    /// The `call` block changed.
    Calls,
    /// Only what the artifact doesn't record can have changed: the plan, checks, examples or budget.
    Source,
    /// The stored artifact's manifest disagrees with its lock entry (D-46).
    Manifest {
        /// The manifest field.
        field: &'static str,
    },
    /// The stored IR no longer validates (`compiler/21` §6).
    Invalid(Vec<Diagnostic>),
}

impl Cause {
    /// Whether the source changed since the goal was built, which `velme build` puts right, rather than the lock or
    /// the stored artifact disagreeing with it.
    pub fn is_source_change(&self) -> bool {
        match self {
            Self::Version { .. }
            | Self::Record { .. }
            | Self::Inputs
            | Self::Output
            | Self::Child { .. }
            | Self::Calls
            | Self::Source => true,
            Self::NotLocked | Self::Manifest { .. } | Self::Invalid(_) => false,
        }
    }
}

/// A version an artifact records (`runtime/32` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Versioned {
    /// The language version.
    Language,
    /// The IR major version.
    Ir,
    /// The builtins major version.
    Builtins,
}

/// How a record type changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordChange {
    /// It gained this field.
    Added(String),
    /// It lost this field.
    Removed(String),
    /// This field has another type.
    Retyped(String),
    /// The same fields, in another order.
    Reordered,
    /// There is no such type any more.
    Gone,
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLocked => write!(f, "it has no entry in `velme.lock`"),
            Self::Version { of, built, now } => {
                let (what, now_is) = match of {
                    Versioned::Language => ("language", "the file says"),
                    Versioned::Ir => ("program format", "this Velme uses"),
                    Versioned::Builtins => ("built-ins", "this Velme has"),
                };
                write!(f, "built for {what} {built}, {now_is} {now}")
            }
            Self::Record { name, change } => match change {
                RecordChange::Added(field) => write!(f, "`{name}` gained a field `{field}`"),
                RecordChange::Removed(field) => write!(f, "`{name}` lost its field `{field}`"),
                RecordChange::Retyped(field) => write!(f, "`{name}`'s field `{field}` changed type"),
                RecordChange::Reordered => write!(f, "`{name}`'s fields changed order"),
                RecordChange::Gone => write!(f, "there is no type `{name}` any more"),
            },
            Self::Inputs => write!(f, "its inputs changed"),
            Self::Output => write!(f, "its output type changed"),
            Self::Child { goal } => write!(f, "`{goal}`, which it calls, changed its inputs or output"),
            Self::Calls => write!(f, "its `call` block changed"),
            Self::Source => write!(f, "its plan, checks, examples or budget changed"),
            Self::Manifest { field } => write!(
                f,
                "the stored artifact doesn't match its own lock entry: its `{field}` differs"
            ),
            Self::Invalid(diags) => {
                let reasons: Vec<String> = diags
                    .iter()
                    .map(|d| {
                        let mut text = d.message.clone();
                        for note in &d.notes {
                            text.push_str(&format!(" ({note})"));
                        }
                        text
                    })
                    .collect();
                write!(f, "the stored artifact is no longer valid: {}", reasons.join("; "))
            }
        }
    }
}
