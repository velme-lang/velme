//! Lowering of the compiler-owned parts of a goal from HIR to IR (`compiler/20` §2): its call section (R-CMP-08) and
//! the types it names.

use std::collections::BTreeMap;

use velme_builtins::Number;
use velme_diagnostics::Diagnostic;
use velme_sema::hir::{self, Expr, ExprKind, GoalId, Program, Type as HirType};

use crate::node::{Call, CallNode, Lambda, Node, Type};
use crate::signature;

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
        HirType::Error => return None,
    })
}

/// A `NUMBER` literal's value in its R-TYP-08 rendering, so `0820.0` and `820` are the same source.
pub(crate) fn number(text: &str) -> Option<String> {
    let (sign, digits) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    // JSON, which `Number::parse` reads, has no leading zeros.
    let digits = digits.trim_start_matches('0');
    let zero = if digits.is_empty() || digits.starts_with('.') {
        "0"
    } else {
        ""
    };
    Number::parse(&format!("{sign}{zero}{digits}")).map(|n| n.to_string())
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
                value: serde_json::from_str(&number(text)?).ok()?,
            },
            ExprKind::Text { value } => Node::Literal {
                ty: Type::Text {},
                value: value.clone().into(),
            },
            ExprKind::Bool { value } => Node::Literal {
                ty: Type::Boolean {},
                value: (*value).into(),
            },
            ExprKind::Nothing => Node::Literal {
                ty: Type::Nothing {},
                value: serde_json::Value::Null,
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
                    of: ir_type(self.program, element)?,
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
