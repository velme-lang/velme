//! Lowering of `check` items and `examples` to the IR expression subset (`language/13` R-CHK-11): the one
//! representation every back end runs, so a check means what the goal's own IR would (INV-3). What is lowered passes
//! the validator's name and type stages before anything evaluates it (INV-1).

use std::collections::BTreeMap;

use serde_json::Value as Json;
use velme_diagnostics::{Diagnostic, Span};
use velme_ir::{BinaryOperator, CheckScope, ItemShape, Lambda, Node, TrustedExpr, Type as IrType, UnaryOperator};
use velme_sema::hir::{BinaryOp, Example, Expr, ExprKind, Goal, Keyword, Program, Quantifier, Type, UnaryOp};

/// The local a lowered check reads the goal's output from (`language/13` R-CHK-02).
pub fn result_local() -> &'static str {
    Keyword::Result.as_str()
}

/// A check item or example value lowered to IR and validated, with the source span of each of its nodes (R-CHK-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowered {
    /// The expression.
    pub node: TrustedExpr,
    /// The span of each node of `node`, in [`Node::preorder`]. A node the lowering adds — the `unwrap_or` of a narrowed
    /// path, a projection's `map`, the `not` of `is not empty` or of `if` — has the span of the expression it lowers,
    /// and comes before that expression's other nodes.
    pub spans: Vec<Span>,
}

/// The check item `check` of `goal`, lowered (R-CHK-11) and validated in `scope`, the goal's check scope. Inputs are
/// `input` nodes, call bindings and `result` are locals, and a quantifier's variable is its lambda's parameter. A
/// narrowed path reads through `unwrap_or`, as IR narrows explicitly (`compiler/21` R-IR-05); its default is never
/// evaluated, since the path is present there.
pub fn lower_check(
    program: &Program,
    goal: &Goal,
    scope: &CheckScope<'_>,
    check: &Expr,
) -> Result<Lowered, Diagnostic> {
    let tree = Lowering::new(program, goal)
        .expr(check)
        .ok_or_else(Diagnostic::internal_error)?;
    tree.lowered(scope, &Type::Boolean)
}

/// An example of `goal`, lowered and validated in `scope`: its arguments, one per parameter, and its expected value
/// (`language/12` R-GOAL-21).
pub fn lower_example(
    program: &Program,
    goal: &Goal,
    scope: &CheckScope<'_>,
    example: &Example,
) -> Result<(Vec<Lowered>, Lowered), Diagnostic> {
    let mut lowering = Lowering::new(program, goal);
    if example.args.len() != goal.params.len() {
        return Err(Diagnostic::internal_error());
    }
    let args = example
        .args
        .iter()
        .zip(&goal.params)
        .map(|(arg, param)| {
            let tree = lowering.expr(arg).ok_or_else(Diagnostic::internal_error)?;
            tree.lowered(scope, &param.ty)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = lowering
        .expr(&example.expected)
        .ok_or_else(Diagnostic::internal_error)?
        .lowered(scope, &goal.output)?;
    Ok((args, expected))
}

/// A node being built, with its spans in [`Node::preorder`].
struct Tree {
    node: Node,
    spans: Vec<Span>,
}

impl Tree {
    /// `node` at `span`, over children whose spans come in [`Node::preorder`]'s child order.
    fn new(span: Span, node: Node, children: Vec<Vec<Span>>) -> Tree {
        let mut spans = vec![span];
        spans.extend(children.into_iter().flatten());
        Tree { node, spans }
    }

    fn leaf(span: Span, node: Node) -> Tree {
        Tree::new(span, node, Vec::new())
    }

    /// The tree validated in `scope` as an expression of a type assignable to `expected`. The source was checked, so
    /// a finding is a lowering bug (R-CHK-11).
    fn lowered(self, scope: &CheckScope<'_>, expected: &Type) -> Result<Lowered, Diagnostic> {
        let node = scope
            .validate(self.node, expected)
            .map_err(|_| Diagnostic::internal_error())?;
        Ok(Lowered {
            node,
            spans: self.spans,
        })
    }
}

/// Splits trees into their nodes and their spans, in order.
fn split(trees: Vec<Tree>) -> (Vec<Node>, Vec<Vec<Span>>) {
    trees.into_iter().map(|t| (t.node, t.spans)).unzip()
}

/// One check or example being lowered. `None` anywhere below means HIR that [`velme_sema::analyze`] doesn't return:
/// an index out of range or a type with an error.
struct Lowering<'p> {
    program: &'p Program,
    goal: &'p Goal,
    /// The quantifier variables in scope, outermost first, with their declared types.
    vars: Vec<(&'p str, Type)>,
    /// Lambda parameters made for projections so far.
    projections: usize,
}

impl<'p> Lowering<'p> {
    fn new(program: &'p Program, goal: &'p Goal) -> Self {
        Lowering {
            program,
            goal,
            vars: Vec::new(),
            projections: 0,
        }
    }

    fn exprs(&mut self, es: &'p [Expr]) -> Option<Vec<Tree>> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    fn expr(&mut self, e: &'p Expr) -> Option<Tree> {
        let span = e.span;
        let tree = match &e.kind {
            ExprKind::Number { text } => Tree::leaf(span, literal(IrType::Number {}, number(text)?)),
            ExprKind::Text { value } => Tree::leaf(span, literal(IrType::Text {}, Json::String(value.clone()))),
            ExprKind::Bool { value } => Tree::leaf(span, literal(IrType::Boolean {}, Json::Bool(*value))),
            ExprKind::Nothing => Tree::leaf(span, literal(IrType::Nothing {}, Json::Null)),
            ExprKind::Input { index } => Tree::leaf(
                span,
                Node::Input {
                    name: self.goal.params.get(*index)?.name.clone(),
                },
            ),
            ExprKind::Binding { index } => Tree::leaf(span, local(&self.goal.bindings.get(*index)?.name)),
            ExprKind::Result => Tree::leaf(span, local(result_local())),
            ExprKind::Var { depth } => Tree::leaf(span, local(self.vars.get(*depth)?.0)),
            ExprKind::Builtin { name, args } => {
                let (args, spans) = split(self.exprs(args)?);
                let node = Node::Builtin {
                    name: (*name).to_owned(),
                    args,
                };
                Tree::new(span, node, spans)
            }
            ExprKind::Record { record, fields } => {
                let declared = self.program.record(*record)?;
                let mut lowered = BTreeMap::new();
                for (def, value) in declared.fields.iter().zip(fields) {
                    lowered.insert(def.name.clone(), self.expr(value)?);
                }
                // Keyed by name, as `Node::preorder` visits them.
                let (names, trees): (Vec<String>, Vec<Tree>) = lowered.into_iter().unzip();
                let (nodes, spans) = split(trees);
                let node = Node::Record {
                    ty: declared.name.clone(),
                    fields: names.into_iter().zip(nodes).collect(),
                };
                Tree::new(span, node, spans)
            }
            ExprKind::List { items } => {
                let Type::List(element) = &e.ty else { return None };
                let (items, spans) = split(self.exprs(items)?);
                let node = Node::List {
                    of: ir_type(self.program, element)?,
                    items,
                };
                Tree::new(span, node, spans)
            }
            ExprKind::Field { base, field } => {
                let field = self.field(&base.ty, *field)?.0;
                let of = self.expr(base)?;
                let node = Node::FieldGet {
                    of: Box::new(of.node),
                    field,
                };
                Tree::new(span, node, vec![of.spans])
            }
            // `xs.field` is `map(xs, e -> e.field)`; the parameter's name is one no source can write, so it hides
            // nothing.
            ExprKind::Project { base, field } => {
                let Type::List(element) = &base.ty else { return None };
                let field = self.field(element, *field)?.0;
                let param = format!("#{}", self.projections);
                self.projections += 1;
                let list = self.expr(base)?;
                let node = Node::Map {
                    items: ItemShape::default(),
                    list: Box::new(list.node),
                    func: Lambda {
                        body: Box::new(Node::FieldGet {
                            of: Box::new(local(&param)),
                            field,
                        }),
                        param,
                    },
                };
                Tree::new(span, node, vec![list.spans, vec![span, span]])
            }
            ExprKind::Unary { op, operand } => {
                let arg = self.expr(operand)?;
                let op = match op {
                    UnaryOp::Neg => UnaryOperator::Neg,
                    UnaryOp::Not => UnaryOperator::Not,
                };
                let node = Node::UnaryOp {
                    op,
                    arg: Box::new(arg.node),
                };
                Tree::new(span, node, vec![arg.spans])
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (left, right) = (self.expr(lhs)?, self.expr(rhs)?);
                let node = Node::BinaryOp {
                    op: binary(*op),
                    left: Box::new(left.node),
                    right: Box::new(right.node),
                };
                Tree::new(span, node, vec![left.spans, right.spans])
            }
            ExprKind::IsEmpty { operand, negated } => {
                let arg = self.expr(operand)?;
                let node = Node::UnaryOp {
                    op: UnaryOperator::IsEmpty,
                    arg: Box::new(arg.node),
                };
                let empty = Tree::new(span, node, vec![arg.spans]);
                if *negated { not(span, empty) } else { empty }
            }
            // `if a then b` is `not a or b`, so `b` is evaluated only when `a` holds.
            ExprKind::If { condition, then } => {
                let (condition, then) = (not(span, self.expr(condition)?), self.expr(then)?);
                let node = Node::BinaryOp {
                    op: BinaryOperator::Or,
                    left: Box::new(condition.node),
                    right: Box::new(then.node),
                };
                Tree::new(span, node, vec![condition.spans, then.spans])
            }
            ExprKind::Quantified {
                quantifier,
                var,
                collection,
                body,
            } => {
                let Type::List(element) = &collection.ty else {
                    return None;
                };
                let list = self.expr(collection)?;
                self.vars.push((var, (**element).clone()));
                let body = self.expr(body);
                self.vars.pop();
                let body = body?;
                let func = Lambda {
                    param: var.clone(),
                    body: Box::new(body.node),
                };
                let items = Box::new(list.node);
                let node = match quantifier {
                    Quantifier::Every => Node::All { list: items, func },
                    Quantifier::Some => Node::Any { list: items, func },
                };
                Tree::new(span, node, vec![list.spans, body.spans])
            }
        };
        if matches!(self.declared(e)?, Some(Type::Optional(_))) && !matches!(e.ty, Type::Optional(_)) {
            let default = literal(ir_type(self.program, &e.ty)?, zero(self.program, &e.ty)?);
            let node = Node::Narrow {
                of: Box::new(tree.node),
                default: Box::new(default),
            };
            return Some(Tree::new(span, node, vec![tree.spans, vec![span]]));
        }
        Some(tree)
    }

    /// The declared type of `e` if it is a path (`language/11` R-TYP-22), before any narrowing.
    fn declared(&self, e: &Expr) -> Option<Option<Type>> {
        Some(Some(match &e.kind {
            ExprKind::Input { index } => self.goal.params.get(*index)?.ty.clone(),
            ExprKind::Binding { index } => self.goal.bindings.get(*index)?.ty.clone(),
            ExprKind::Result => self.goal.output.clone(),
            ExprKind::Var { depth } => self.vars.get(*depth)?.1.clone(),
            ExprKind::Field { base, field } => self.field(&base.ty, *field)?.1,
            _ => return Some(None),
        }))
    }

    /// The name and type of field `index` of the record type `record`.
    fn field(&self, record: &Type, index: usize) -> Option<(String, Type)> {
        let Type::Record(id) = record else { return None };
        let field = self.program.record(*id)?.fields.get(index)?;
        Some((field.name.clone(), field.ty.clone()))
    }
}

fn literal(ty: IrType, value: Json) -> Node {
    Node::Literal {
        ty,
        value: value.into(),
    }
}

fn local(name: &str) -> Node {
    Node::Local { name: name.to_owned() }
}

/// `not arg`, at `span`.
fn not(span: Span, arg: Tree) -> Tree {
    let node = Node::UnaryOp {
        op: UnaryOperator::Not,
        arg: Box::new(arg.node),
    };
    Tree::new(span, node, vec![arg.spans])
}

fn binary(op: BinaryOp) -> BinaryOperator {
    match op {
        BinaryOp::Or => BinaryOperator::Or,
        BinaryOp::And => BinaryOperator::And,
        BinaryOp::Eq => BinaryOperator::Eq,
        BinaryOp::NotEq => BinaryOperator::Ne,
        BinaryOp::Lt => BinaryOperator::Lt,
        BinaryOp::LtEq => BinaryOperator::Le,
        BinaryOp::Gt => BinaryOperator::Gt,
        BinaryOp::GtEq => BinaryOperator::Ge,
        BinaryOp::Add => BinaryOperator::Add,
        BinaryOp::Sub => BinaryOperator::Sub,
        BinaryOp::Mul => BinaryOperator::Mul,
        BinaryOp::Div => BinaryOperator::Div,
    }
}

/// A `NUMBER` literal's text as a JSON number: the same digits, without the leading zeros JSON forbids.
fn number(text: &str) -> Option<Json> {
    let (sign, digits) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    let trimmed = digits.trim_start_matches('0');
    let digits = if trimmed.is_empty() || trimmed.starts_with('.') {
        format!("0{trimmed}")
    } else {
        trimmed.to_owned()
    };
    serde_json::from_str(&format!("{sign}{digits}")).ok()
}

/// `ty` as IR (`compiler/21` §2.1).
fn ir_type(program: &Program, ty: &Type) -> Option<IrType> {
    Some(match ty {
        Type::Number => IrType::Number {},
        Type::Text => IrType::Text {},
        Type::Boolean => IrType::Boolean {},
        Type::Nothing => IrType::Nothing {},
        Type::Optional(inner) => IrType::Optional {
            of: Box::new(ir_type(program, inner)?),
        },
        Type::List(element) => IrType::List {
            of: Box::new(ir_type(program, element)?),
        },
        Type::Record(id) => IrType::Record {
            name: program.record(*id)?.name.clone(),
        },
        Type::Error => return None,
    })
}

/// Some value of `ty`, as the JSON of a literal: the never-evaluated default of a narrowing `unwrap_or`. Records are
/// never recursive (R-TYP-18), so this ends.
fn zero(program: &Program, ty: &Type) -> Option<Json> {
    Some(match ty {
        Type::Number => Json::from(0),
        Type::Text => Json::String(String::new()),
        Type::Boolean => Json::Bool(false),
        Type::Nothing | Type::Optional(_) => Json::Null,
        Type::List(_) => Json::Array(Vec::new()),
        Type::Record(id) => Json::Object(
            program
                .record(*id)?
                .fields
                .iter()
                .map(|f| Some((f.name.clone(), zero(program, &f.ty)?)))
                .collect::<Option<_>>()?,
        ),
        Type::Error => return None,
    })
}
