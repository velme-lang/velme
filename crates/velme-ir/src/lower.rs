//! Lowering of the compiler-owned parts of a goal from HIR to IR (`compiler/20` §2): its call section (R-CMP-08) and
//! the types it names.

use std::collections::BTreeMap;

use velme_diagnostics::Diagnostic;
use velme_sema::hir::{self, Expr, ExprKind, GoalId, Program, Type as HirType};

use crate::fingerprint::tables;
use crate::node::{Bound, Call, CallNode, Domain, Goal, ItemShape, Lambda, Length, Node, RecordType, Type};
use crate::{IR_VERSION, signature};

/// The call section of `goal`: one `call` node per binding, in source order, each with its child's signature
/// (`compiler/21` R-IR-09); empty for a leaf goal. It is what a composite candidate is joined with (R-CMP-08) and what a
/// complete goal's `calls` must equal (R-IR-16).
pub fn calls(program: &Program, goal: GoalId) -> Result<Vec<CallNode>, Diagnostic> {
    let goal = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    let mut lowering = Args {
        program,
        goal,
        projections: 0,
    };
    goal.bindings
        .iter()
        .map(|binding| {
            let callee = program
                .goals
                .get(binding.callee.0)
                .ok_or_else(Diagnostic::internal_error)?;
            let args = binding
                .args
                .iter()
                .map(|arg| lowering.arg(arg))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(Diagnostic::internal_error)?;
            Ok(CallNode::Call(Call {
                binding: binding.name.clone(),
                goal: callee.name.clone(),
                goal_signature: signature(program, binding.callee)?.to_string(),
                args,
            }))
        })
        .collect()
}

/// The whole IR of the wired goal `goal` (`language/12` R-GOAL-12, D-4): the compiler's `calls`, and as `body` the
/// binding `result`, which is the goal's output (`compiler/21` R-IR-03). Nothing is synthesized for it, so it has no
/// provider and needs none.
pub fn wired_goal(program: &Program, goal: GoalId) -> Result<Goal, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    let (types, domains) = goal_tables(program, goal)?;
    let inputs = target
        .params
        .iter()
        .map(|p| {
            Ok((
                p.name.clone(),
                ir_type(program, &p.ty).ok_or_else(Diagnostic::internal_error)?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    Ok(Goal {
        ir_version: IR_VERSION.to_owned(),
        builtins_version: velme_builtins::BUILTINS_VERSION.to_owned(),
        goal: target.name.clone(),
        types,
        domains,
        inputs,
        output: ir_type(program, &target.output).ok_or_else(Diagnostic::internal_error)?,
        calls: calls(program, goal)?,
        body: Node::Local {
            name: "result".to_owned(),
        },
    })
}

/// An IR goal's `types` and `domains`, by name (`compiler/21` R-IR-01, R-IR-24).
pub type Tables = (BTreeMap<String, RecordType>, BTreeMap<String, Domain>);

/// The IR's `types` and `domains` of `goal`, by name: the records and domains its inputs, output and calls' signatures
/// reach (`compiler/21` R-IR-01, R-IR-24). What the compiler writes, and a candidate is completed with (`compiler/22`
/// R-SYNTH-10).
pub fn goal_tables(program: &Program, goal: GoalId) -> Result<Tables, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    let mut roots: Vec<&HirType> = target.params.iter().map(|p| &p.ty).chain([&target.output]).collect();
    for binding in &target.bindings {
        let callee = program
            .goals
            .get(binding.callee.0)
            .ok_or_else(Diagnostic::internal_error)?;
        roots.extend(callee.params.iter().map(|p| &p.ty).chain([&callee.output]));
    }
    tables(program, roots).ok_or_else(Diagnostic::internal_error)
}

/// `ty` as IR (`compiler/21` §2.1); `None` for a type with an error, which a checked program never has.
pub fn ir_type(program: &Program, ty: &HirType) -> Option<Type> {
    Some(match ty {
        HirType::Number => Type::Number {},
        HirType::Text => Type::Text {},
        HirType::Boolean => Type::Boolean {},
        HirType::Nothing => Type::Nothing {},
        HirType::Optional(inner) => Type::Optional {
            of: Box::new(ir_type(program, inner)?),
        },
        HirType::List(element) => Type::List {
            of: Box::new(ir_type(program, element)?),
        },
        HirType::Record(id) => Type::Record {
            name: program.record(*id)?.name.clone(),
        },
        HirType::Domain(id, _) => Type::Domain {
            name: program.domain(*id)?.name.clone(),
        },
        HirType::Error => return None,
    })
}

/// `ty` as a body annotation (`literal.type`, `list.of`): its IR type with every domain replaced by its base, since a
/// body never names a Domain (`compiler/21` R-IR-25).
pub fn body_type(program: &Program, ty: &HirType) -> Option<Type> {
    ir_type(program, &ty.erased())
}

/// The `domains` entry of `domain` (`compiler/21` R-IR-24): its base and the simple constraints extracted from its
/// condition (R-CHK-14), bounds as their exact values; residual conditions stay in HIR (R-CHK-15).
pub(crate) fn domain_entry(program: &Program, domain: &hir::DomainType) -> Option<Domain> {
    let bound = |b: &Option<hir::Bound>| -> Option<Option<Bound>> {
        let Some(b) = b else { return Some(None) };
        Some(Some(Bound {
            value: serde_json::from_str(&number(&b.value)?).ok()?,
            inclusive: b.inclusive,
        }))
    };
    let c = &domain.constraints;
    let members = match &domain.rule {
        hir::DomainRule::OneOf { members } => Some(members.clone()),
        hir::DomainRule::Where { .. } => None,
    };
    let (min, max) = (bound(&c.min_length)?, bound(&c.max_length)?);
    Some(Domain {
        base: ir_type(program, &domain.base)?,
        lower: bound(&c.lower)?,
        upper: bound(&c.upper)?,
        whole: c.whole,
        members,
        length: (min.is_some() || max.is_some()).then_some(Length { lower: min, upper: max }),
    })
}

/// A `NUMBER` literal's value in its R-TYP-08 rendering, so `0820.0` and `820` are the same source.
pub(crate) fn number(text: &str) -> Option<String> {
    velme_sema::number(text).map(|n| n.to_string())
}

/// The arguments of one goal's calls being lowered. `None` anywhere below means HIR that [`velme_sema::analyze`]
/// doesn't return: an index out of range, a type with an error, or an argument R-GOAL-08 rejects.
struct Args<'p> {
    program: &'p Program,
    goal: &'p hir::Goal,
    /// Lambda parameters made for projections so far, across the whole call section, so each is unique (R-IR-12).
    projections: usize,
}

impl Args<'_> {
    /// A path from an input or an earlier binding, or a literal (`language/12` R-GOAL-08). A call can't narrow, so no
    /// argument reads through `unwrap_or`.
    fn arg(&mut self, e: &Expr) -> Option<Node> {
        Some(match &e.kind {
            ExprKind::Number { text } => Node::Literal {
                ty: Type::Number {},
                value: serde_json::from_str::<serde_json::Value>(&number(text)?).ok()?.into(),
            },
            ExprKind::Text { value } => Node::Literal {
                ty: Type::Text {},
                value: serde_json::Value::from(value.clone()).into(),
            },
            ExprKind::Bool { value } => Node::Literal {
                ty: Type::Boolean {},
                value: serde_json::Value::from(*value).into(),
            },
            ExprKind::Nothing => Node::Literal {
                ty: Type::Nothing {},
                value: serde_json::Value::Null.into(),
            },
            ExprKind::Input { index } => Node::Input {
                name: self.goal.params.get(*index)?.name.clone(),
            },
            ExprKind::Binding { index } => Node::Local {
                name: self.goal.bindings.get(*index)?.name.clone(),
            },
            ExprKind::Record { record, fields } => {
                let declared = self.program.record(*record)?;
                let mut lowered = BTreeMap::new();
                for (def, value) in declared.fields.iter().zip(fields) {
                    lowered.insert(def.name.clone(), self.arg(value)?);
                }
                Node::Record {
                    ty: declared.name.clone(),
                    fields: lowered,
                }
            }
            ExprKind::List { items } => {
                let HirType::List(element) = &e.ty else { return None };
                Node::List {
                    of: body_type(self.program, element)?,
                    items: items.iter().map(|item| self.arg(item)).collect::<Option<_>>()?,
                }
            }
            ExprKind::Field { base, field } => Node::FieldGet {
                field: self.field_name(&base.ty, *field)?,
                of: Box::new(self.arg(base)?),
            },
            // `xs.field` is `map(xs, e -> e.field)`, as a lowered check writes it; the parameter's name is one no
            // source can write, so it hides nothing.
            ExprKind::Project { base, field } => {
                let HirType::List(element) = &base.ty else { return None };
                let field = self.field_name(element, *field)?;
                let param = format!("#{}", self.projections);
                self.projections += 1;
                Node::Map {
                    items: ItemShape::default(),
                    list: Box::new(self.arg(base)?),
                    func: Lambda {
                        body: Box::new(Node::FieldGet {
                            of: Box::new(Node::Local { name: param.clone() }),
                            field,
                        }),
                        param,
                    },
                }
            }
            _ => return None,
        })
    }

    /// The name of field `index` of the record type `ty`.
    fn field_name(&self, ty: &HirType, index: usize) -> Option<String> {
        let HirType::Record(id) = ty else { return None };
        Some(self.program.record(*id)?.fields.get(index)?.name.clone())
    }
}
