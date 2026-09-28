//! Phase 4, type-check (`compiler/20` §3): each goal's `check` items and `examples` against the signatures
//! (`language/11` §8–9, `language/13` §2–3, `language/12` R-GOAL-21). The `call` block is checked with the call graph.

use std::collections::{BTreeMap, BTreeSet};

use velme_builtins::{Builtin, Shape};
use velme_diagnostics::{Code, Diagnostic, Span, closest};
use velme_syntax::ast::{self, BinaryOp, ExprKind as Ast, LiteralKind, UnaryOp};

use crate::hir::{Example, Expr, ExprKind, Goal, RecordType, Type, TypeId};
use crate::resolve::{Declared, Scope};

/// Checks every goal's body and fills in its checks and examples. Goals whose name is taken are checked too
/// (CC-ERR-04).
pub(crate) fn check_bodies(
    types: &[RecordType],
    goals: &mut [Goal],
    scope: &Scope<'_>,
    text: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let file = File {
        types,
        goals,
        scope,
        text,
    };
    for (decl, goal) in &scope.unnamed_goals {
        drop(file.body(decl, goal, diags));
    }
    let bodies: Vec<_> = scope
        .goals
        .iter()
        .zip(goals.iter())
        .map(|(decl, goal)| file.body(decl, goal, diags))
        .collect();
    for (goal, (checks, examples)) in goals.iter_mut().zip(bodies) {
        goal.checks = checks;
        goal.examples = examples;
    }
}

/// What every body sees.
struct File<'f> {
    types: &'f [RecordType],
    goals: &'f [Goal],
    scope: &'f Scope<'f>,
    text: &'f str,
}

impl File<'_> {
    fn body(&self, decl: &ast::GoalDecl, goal: &Goal, diags: &mut Vec<Diagnostic>) -> (Vec<Expr>, Vec<Example>) {
        // A binding's type is its callee's output (R-GOAL-12); calls themselves are checked with the call graph.
        let bindings = decl
            .call
            .iter()
            .flat_map(|c| &c.bindings)
            .filter(|b| b.name.name != "result")
            .map(|b| {
                let ty = match self.scope.names.get(b.callee.name.as_str()) {
                    Some(Declared::Goal(id)) => self.goals.get(id.0).map_or(Type::Error, |g| g.output.clone()),
                    _ => Type::Error,
                };
                (b.name.name.as_str(), ty)
            })
            .collect();
        let mut body = Body {
            file: self,
            goal,
            bindings,
            vars: Vec::new(),
            narrowed: Vec::new(),
            diags,
        };
        let checks = decl
            .check
            .iter()
            .flat_map(|c| &c.items)
            .map(|item| body.check_item(item))
            .collect();
        let examples = decl
            .examples
            .iter()
            .flat_map(|e| &e.items)
            .filter_map(|e| body.example(e))
            .collect();
        (checks, examples)
    }
}

/// Where a path starts (R-TYP-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Root {
    Input(usize),
    Binding(usize),
    Result,
    Var(usize),
}

impl Root {
    fn kind(self) -> ExprKind {
        match self {
            Root::Input(index) => ExprKind::Input { index },
            Root::Binding(index) => ExprKind::Binding { index },
            Root::Result => ExprKind::Result,
            Root::Var(depth) => ExprKind::Var { depth },
        }
    }
}

/// A name followed by field accesses, the only thing narrowing applies to (R-TYP-22).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Path<'a> {
    root: Root,
    fields: Vec<&'a str>,
}

/// One goal's body being checked.
struct Body<'f, 'a> {
    file: &'f File<'f>,
    goal: &'f Goal,
    /// `call` bindings visible in checks, in block order.
    bindings: Vec<(&'a str, Type)>,
    /// Quantifier variables in scope, outermost first.
    vars: Vec<(&'a str, Type)>,
    /// Paths known to be present here (`language/11` §9).
    narrowed: Vec<Path<'a>>,
    diags: &'f mut Vec<Diagnostic>,
}

/// A check expression or example literal, both of which can be list and record items.
trait Value {
    fn check<'a>(&'a self, body: &mut Body<'_, 'a>, expected: Option<&Type>) -> Expr;
}

impl Value for ast::Expr {
    fn check<'a>(&'a self, body: &mut Body<'_, 'a>, expected: Option<&Type>) -> Expr {
        body.expr(self, expected)
    }
}

impl Value for ast::Literal {
    fn check<'a>(&'a self, body: &mut Body<'_, 'a>, expected: Option<&Type>) -> Expr {
        body.literal(self, expected)
    }
}

/// A value whose error was already reported.
fn poisoned(span: Span) -> Expr {
    Expr {
        kind: ExprKind::Nothing,
        ty: Type::Error,
        span,
    }
}

impl<'f, 'a> Body<'f, 'a> {
    /// R-CHK-01: a check item is `Boolean`.
    fn check_item(&mut self, item: &'a ast::Expr) -> Expr {
        let e = self.expr(item, None);
        self.expect_boolean(&e, "a check must be true or false, like `result > 0`");
        e
    }

    /// R-GOAL-21: `Goal(literal, …) == literal`, calling the goal it belongs to.
    fn example(&mut self, example: &'a ast::Example) -> Option<Example> {
        let goal = self.goal;
        let args = if example.goal.name != goal.name {
            self.diags.push(
                Diagnostic::new(
                    Code::InvalidCall,
                    example.goal.span,
                    format!("`{}` can only be used after it's listed in `call:`.", example.goal.name),
                )
                .with_help(format!("an example calls the goal it belongs to, `{}`", goal.name)),
            );
            None
        } else if example.args.len() != goal.params.len() {
            self.diags
                .push(arity(&goal.name, goal.params.len(), example.args.len(), example.span));
            None
        } else {
            Some(
                example
                    .args
                    .iter()
                    .zip(&goal.params)
                    .map(|(arg, param)| self.assigned(arg, &param.ty))
                    .collect(),
            )
        };
        let Some(args) = args else {
            for arg in example.args.iter().chain([&example.expected]) {
                self.literal(arg, Some(&Type::Error));
            }
            return None;
        };
        let expected = self.assigned(&example.expected, &goal.output);
        Some(Example {
            args,
            expected,
            span: example.span,
        })
    }

    /// `value` checked against `ty`, which it must be assignable to (R-TYP-20).
    fn assigned<V: Value>(&mut self, value: &'a V, ty: &Type) -> Expr {
        let e = value.check(self, Some(ty));
        if !assignable(&e.ty, ty) {
            let diag = self.mismatch(&e, &[self.type_name(ty)]);
            self.diags.push(diag);
        }
        e
    }

    fn literal(&mut self, literal: &'a ast::Literal, expected: Option<&Type>) -> Expr {
        let (kind, ty) = match &literal.kind {
            LiteralKind::Number { text } => (ExprKind::Number { text: text.clone() }, Type::Number),
            LiteralKind::Text { value } => (ExprKind::Text { value: value.clone() }, Type::Text),
            LiteralKind::Bool { value } => (ExprKind::Bool { value: *value }, Type::Boolean),
            LiteralKind::Nothing => (ExprKind::Nothing, Type::Nothing),
            LiteralKind::List { items } => self.list(items, expected, literal.span),
            LiteralKind::Record { name, fields } => {
                let fields: Vec<_> = fields.iter().map(|f| (&f.name, &f.value)).collect();
                match self.file.scope.names.get(name.name.as_str()) {
                    Some(&Declared::Type(id)) => self.record(id, &fields, literal.span),
                    Some(Declared::Goal(_)) => {
                        self.diags.push(
                            Diagnostic::new(
                                Code::UnknownType,
                                name.span,
                                format!("`{}` is a goal, not a type.", name.name),
                            )
                            .with_help(
                                "an example's values are literals, like `3` or a record `Player(name: \"Ada\")`",
                            ),
                        );
                        self.skip(fields.iter().map(|(_, v)| *v));
                        return poisoned(literal.span);
                    }
                    None => {
                        if !self.file.scope.is_failed(&name.name) {
                            let diag = self.unknown_type(name);
                            self.diags.push(diag);
                        }
                        self.skip(fields.iter().map(|(_, v)| *v));
                        return poisoned(literal.span);
                    }
                }
            }
        };
        Expr {
            kind,
            ty,
            span: literal.span,
        }
    }

    fn expr(&mut self, e: &'a ast::Expr, expected: Option<&Type>) -> Expr {
        self.expr_narrowed(e, expected, true)
    }

    /// `e`, narrowed only if `narrow`: `==`, `!=` and `is empty` see a path's declared type, so they can test it
    /// again.
    fn expr_narrowed(&mut self, e: &'a ast::Expr, expected: Option<&Type>, narrow: bool) -> Expr {
        let (kind, ty) = match &e.kind {
            Ast::Number { text } => (ExprKind::Number { text: text.clone() }, Type::Number),
            Ast::Text { value } => (ExprKind::Text { value: value.clone() }, Type::Text),
            Ast::Bool { value } => (ExprKind::Bool { value: *value }, Type::Boolean),
            Ast::Nothing => (ExprKind::Nothing, Type::Nothing),
            Ast::Result => (ExprKind::Result, self.goal.output.clone()),
            Ast::Name { name } => match self.lookup(name) {
                Some((root, ty)) => (root.kind(), ty),
                None => {
                    self.unknown_name(name, e.span);
                    return poisoned(e.span);
                }
            },
            Ast::Call { callee, args } => self.call(callee, args, e.span),
            Ast::List { items } => self.list(items, expected, e.span),
            Ast::Field { base, field } => self.field(base, field),
            Ast::Unary { op, operand } => self.unary(*op, operand),
            Ast::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            Ast::IsEmpty { operand, negated } => {
                let operand = self.expr_narrowed(operand, None, false);
                self.check_emptiable(&operand, *negated);
                (
                    ExprKind::IsEmpty {
                        operand: Box::new(operand),
                        negated: *negated,
                    },
                    Type::Boolean,
                )
            }
            Ast::If { condition, then } => {
                let condition_e = self.expr(condition, None);
                self.expect_boolean(&condition_e, "a condition must be true or false, like `score > 0`");
                let then = self.narrowed(condition, true, |b| b.expr(then, None));
                self.expect_boolean(&then, "what follows `then` must be true or false");
                (
                    ExprKind::If {
                        condition: Box::new(condition_e),
                        then: Box::new(then),
                    },
                    Type::Boolean,
                )
            }
            Ast::Quantified {
                quantifier,
                var,
                collection,
                body,
            } => self.quantified(*quantifier, var, collection, body),
        };
        let ty = if narrow { self.narrow(e, ty) } else { ty };
        Expr { kind, ty, span: e.span }
    }

    /// `ty` of the path `e`, with `?` removed if the path is narrowed here.
    fn narrow(&self, e: &'a ast::Expr, ty: Type) -> Type {
        match ty {
            Type::Optional(inner) if self.path(e).is_some_and(|p| self.narrowed.contains(&p)) => *inner,
            ty => ty,
        }
    }

    /// The value names in scope: quantifier variables (innermost first), inputs, then bindings (R-CHK-02).
    fn lookup(&self, name: &str) -> Option<(Root, Type)> {
        if let Some((depth, (_, ty))) = self.vars.iter().enumerate().rev().find(|(_, (n, _))| *n == name) {
            return Some((Root::Var(depth), ty.clone()));
        }
        if let Some((i, p)) = self.goal.params.iter().enumerate().find(|(_, p)| p.name == name) {
            return Some((Root::Input(i), p.ty.clone()));
        }
        let (i, (_, ty)) = self.bindings.iter().enumerate().find(|(_, (n, _))| *n == name)?;
        Some((Root::Binding(i), ty.clone()))
    }

    fn path(&self, e: &'a ast::Expr) -> Option<Path<'a>> {
        match &e.kind {
            Ast::Result => Some(Path {
                root: Root::Result,
                fields: Vec::new(),
            }),
            Ast::Name { name } => self.lookup(name).map(|(root, _)| Path {
                root,
                fields: Vec::new(),
            }),
            Ast::Field { base, field } => {
                let mut path = self.path(base)?;
                path.fields.push(&field.name);
                Some(path)
            }
            _ => None,
        }
    }

    /// The paths `condition` proves present when it evaluates to `when` (R-TYP-22, R-TYP-27).
    fn narrowings(&self, condition: &'a ast::Expr, when: bool, out: &mut Vec<Path<'a>>) {
        match &condition.kind {
            Ast::IsEmpty { operand, negated } if *negated == when => out.extend(self.path(operand)),
            Ast::Binary {
                op: op @ (BinaryOp::Eq | BinaryOp::NotEq),
                lhs,
                rhs,
            } if (*op == BinaryOp::NotEq) == when => match (&lhs.kind, &rhs.kind) {
                (_, Ast::Nothing) => out.extend(self.path(lhs)),
                (Ast::Nothing, _) => out.extend(self.path(rhs)),
                _ => {}
            },
            Ast::Binary {
                op: BinaryOp::And,
                lhs,
                rhs,
            } if when => {
                self.narrowings(lhs, true, out);
                self.narrowings(rhs, true, out);
            }
            Ast::Binary {
                op: BinaryOp::Or,
                lhs,
                rhs,
            } if !when => {
                self.narrowings(lhs, false, out);
                self.narrowings(rhs, false, out);
            }
            Ast::Unary {
                op: UnaryOp::Not,
                operand,
            } => self.narrowings(operand, !when, out),
            // `if a then b` is `not a or b`.
            Ast::If { condition, then } if !when => {
                self.narrowings(condition, true, out);
                self.narrowings(then, false, out);
            }
            _ => {}
        }
    }

    /// `f`, with the paths `condition` proves present when it is `when`.
    fn narrowed<R>(&mut self, condition: &'a ast::Expr, when: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = self.narrowed.len();
        let mut paths = Vec::new();
        self.narrowings(condition, when, &mut paths);
        self.narrowed.extend(paths);
        let out = f(self);
        self.narrowed.truncate(before);
        out
    }

    /// `Name(args)`: a record literal, a built-in call, or a mistake (R-SYN-16, R-CHK-03).
    fn call(&mut self, callee: &'a ast::Ident, args: &'a [ast::Arg], span: Span) -> (ExprKind, Type) {
        let name = callee.name.as_str();
        let declared = self.file.scope.names.get(name).copied();
        // A check never calls a goal, so a built-in's name calls the built-in even if a goal has it; a type of that
        // name is a record literal only with named fields (R-BLT-12, D-64).
        let named = !args.is_empty() && args.iter().all(|a| a.name.is_some());
        if let Some(builtin) = Builtin::find(name).filter(|b| b.in_checks)
            && !(named && matches!(declared, Some(Declared::Type(_))))
        {
            return self.builtin(builtin, args, span);
        }
        match declared {
            Some(Declared::Type(id)) => {
                let named: Option<Vec<_>> = args.iter().map(|a| a.name.as_ref().map(|n| (n, &a.value))).collect();
                if let Some(fields) = named {
                    return self.record(id, &fields, span);
                }
                self.diags.push(
                    Diagnostic::new(
                        Code::UnexpectedToken,
                        span,
                        format!("Each value in `{name}(…)` needs its field name."),
                    )
                    .with_help(self.field_names_help(id)),
                );
                self.skip(args.iter().map(|a| &a.value));
                (ExprKind::Nothing, Type::Record(id))
            }
            Some(Declared::Goal(_)) => {
                self.diags.push(
                    Diagnostic::new(
                        Code::InvalidCall,
                        callee.span,
                        format!("`{name}` can only be used after it's listed in `call:`."),
                    )
                    .with_help("a check looks at one run; list the call in `call:` and use its name here"),
                );
                self.skip(args.iter().map(|a| &a.value));
                (ExprKind::Nothing, Type::Error)
            }
            None => match Builtin::find(name) {
                _ if self.file.scope.is_failed(name) => {
                    self.skip(args.iter().map(|a| &a.value));
                    (ExprKind::Nothing, Type::Error)
                }
                found => {
                    let mut diag = Diagnostic::new(
                        Code::UnknownName,
                        callee.span,
                        format!("I don't know what `{name}` is here."),
                    );
                    if found.is_some() {
                        diag = diag.with_help(format!(
                            "`{name}` works only in generated code; in a check, use `every … in … has …` or \
                             `some … in … has …`"
                        ));
                    } else {
                        let candidates = velme_builtins::CATALOG
                            .iter()
                            .filter(|b| b.in_checks)
                            .map(|b| b.name)
                            .chain(self.type_names());
                        if let Some(suggestion) = closest(name, candidates) {
                            diag = diag.with_help(format!("did you mean `{suggestion}`?"));
                        }
                    }
                    self.diags.push(diag);
                    self.skip(args.iter().map(|a| &a.value));
                    (ExprKind::Nothing, Type::Error)
                }
            },
        }
    }

    /// A call to a built-in allowed in checks (`language/14` §2), matched against its signatures in order.
    fn builtin(&mut self, builtin: &'static Builtin, args: &'a [ast::Arg], span: Span) -> (ExprKind, Type) {
        if let Some(named) = args.iter().find_map(|a| a.name.as_ref()) {
            self.diags.push(
                Diagnostic::new(
                    Code::UnexpectedToken,
                    named.span,
                    format!("`{}` takes its inputs without names.", builtin.name),
                )
                .with_help(format!("remove `{}:`", named.name)),
            );
        }
        let candidates: Vec<_> = builtin
            .signatures
            .iter()
            .filter(|s| s.params.len() == args.len())
            .collect();
        let Some(first) = candidates.first() else {
            let expected = builtin.signatures.first().map_or(0, |s| s.params.len());
            self.diags.push(arity(builtin.name, expected, args.len(), span));
            self.skip(args.iter().map(|a| &a.value));
            return (ExprKind::Nothing, Type::Error);
        };
        // Arguments are checked once, in order; an earlier one fixes `T` for a later one, as in `contains(xs, [])`.
        let mut vars = Vars::default();
        let args: Vec<Expr> = args
            .iter()
            .zip(first.params)
            .map(|(arg, shape)| {
                let e = self.expr(&arg.value, vars.instantiate(shape).as_ref());
                vars.unify(shape, &e.ty);
                e
            })
            .collect();
        for signature in &candidates {
            let mut vars = Vars::default();
            if signature.params.iter().zip(&args).all(|(s, a)| vars.unify(s, &a.ty)) {
                let ty = vars.instantiate(&signature.output).unwrap_or(Type::Error);
                return (
                    ExprKind::Builtin {
                        name: builtin.name,
                        args,
                    },
                    ty,
                );
            }
        }
        let mut vars = Vars::default();
        let bad = first
            .params
            .iter()
            .zip(&args)
            .position(|(s, a)| !vars.unify(s, &a.ty))
            .unwrap_or(0);
        let mut wanted: Vec<String> = Vec::new();
        for shape in candidates.iter().filter_map(|s| s.params.get(bad)) {
            let described = vars.describe(shape, self.file.types);
            if !wanted.contains(&described) {
                wanted.push(described);
            }
        }
        if let Some(arg) = args.get(bad) {
            let diag = self.mismatch(arg, &wanted);
            self.diags.push(diag);
        }
        (
            ExprKind::Builtin {
                name: builtin.name,
                args,
            },
            Type::Error,
        )
    }

    /// `Type(field: value, …)` (R-TYP-17): every field exactly once, each value assignable to its field.
    fn record<V: Value>(&mut self, id: TypeId, fields: &[(&'a ast::Ident, &'a V)], span: Span) -> (ExprKind, Type) {
        let Some(record) = self.file.types.get(id.0) else {
            return (ExprKind::Nothing, Type::Error);
        };
        let mut values: Vec<Option<Expr>> = vec![None; record.fields.len()];
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut unknown = false;
        for &(name, value) in fields {
            let Some((index, field)) = record.fields.iter().enumerate().find(|(_, f)| f.name == name.name) else {
                unknown = true;
                let diag = self.unknown_field(&record.name, name, record.fields.iter().map(|f| f.name.as_str()));
                self.diags.push(diag);
                value.check(self, Some(&Type::Error));
                continue;
            };
            let e = self.assigned(value, &field.ty);
            if !seen.insert(&name.name) {
                self.diags.push(
                    Diagnostic::new(
                        Code::DuplicateDeclaration,
                        name.span,
                        format!("`{}` is given a value twice.", name.name),
                    )
                    .with_help("remove one of them"),
                );
                continue;
            }
            if let Some(slot) = values.get_mut(index) {
                *slot = Some(e);
            }
        }
        let missing: Vec<String> = record
            .fields
            .iter()
            .zip(&values)
            .filter(|(_, v)| v.is_none())
            .map(|(f, _)| format!("`{}`", f.name))
            .collect();
        // A misspelled field is reported once, as unknown, not again as missing.
        if !missing.is_empty() && !unknown {
            self.diags.push(
                Diagnostic::new(
                    Code::TypeMismatch,
                    span,
                    format!("This `{}` is missing {}.", record.name, and_list(&missing)),
                )
                .with_help("give every field a value; a field whose type ends in `?` can be `nothing`"),
            );
        }
        (
            ExprKind::Record {
                ty: id,
                fields: values.into_iter().flatten().collect(),
            },
            Type::Record(id),
        )
    }

    /// `[a, b]` (R-TYP-13, R-TYP-26): the expected list type if every item fits it, else the items' common type.
    fn list<V: Value>(&mut self, items: &'a [V], expected: Option<&Type>, span: Span) -> (ExprKind, Type) {
        let element = match expected {
            Some(Type::List(element)) => Some(&**element),
            Some(Type::Optional(inner)) => match &**inner {
                Type::List(element) => Some(&**element),
                _ => None,
            },
            Some(Type::Error) => Some(&Type::Error),
            _ => None,
        };
        let items: Vec<Expr> = items.iter().map(|item| item.check(self, element)).collect();
        if let Some(element) = element
            && items.iter().all(|item| assignable(&item.ty, element))
        {
            let ty = Type::List(Box::new(element.clone()));
            return (ExprKind::List { items }, ty);
        }
        let mut common: Option<Type> = None;
        let mut clash = None;
        for item in &items {
            let Some(so_far) = common.take() else {
                common = Some(item.ty.clone());
                continue;
            };
            let Some(joined) = join(&so_far, &item.ty) else {
                clash = Some(self.mismatch(item, &[self.type_name(&so_far)]));
                break;
            };
            common = Some(joined);
        }
        if let Some(diag) = clash {
            self.diags.push(diag.with_help("a list holds one type of value"));
            return (ExprKind::List { items }, Type::Error);
        }
        let Some(element) = common else {
            self.diags.push(
                Diagnostic::new(Code::TypeMismatch, span, "I can't tell what kind of list this is.")
                    .with_help("compare the list with a value whose type is known, or use `is empty`"),
            );
            return (ExprKind::List { items }, Type::Error);
        };
        (ExprKind::List { items }, Type::List(Box::new(element)))
    }

    /// `base.field`: a record field, a list's or text's `.length`, or a projection over a list of records
    /// (R-TYP-14, R-TYP-12).
    fn field(&mut self, base: &'a ast::Expr, field: &'a ast::Ident) -> (ExprKind, Type) {
        let base_e = self.expr(base, None);
        let name = field.name.as_str();
        let base_ty = base_e.ty.clone();
        let length = |base_e: Expr| {
            (
                ExprKind::Builtin {
                    name: "length",
                    args: vec![base_e],
                },
                Type::Number,
            )
        };
        match &base_ty {
            Type::Error => (ExprKind::Nothing, Type::Error),
            Type::Optional(_) => {
                self.nullable(base);
                (ExprKind::Nothing, Type::Error)
            }
            Type::Text | Type::List(_) if name == "length" => length(base_e),
            Type::Record(id) => match self.record_field(*id, field) {
                Some((index, ty)) => (
                    ExprKind::Field {
                        base: Box::new(base_e),
                        field: index,
                    },
                    ty,
                ),
                None => (ExprKind::Nothing, Type::Error),
            },
            Type::List(element) => match &**element {
                Type::Record(id) => match self.record_field(*id, field) {
                    Some((index, ty)) => (
                        ExprKind::Project {
                            base: Box::new(base_e),
                            field: index,
                        },
                        Type::List(Box::new(ty)),
                    ),
                    None => (ExprKind::Nothing, Type::Error),
                },
                Type::Optional(inner) if matches!(**inner, Type::Record(_)) => {
                    let list = self.source(base.span);
                    self.diags.push(
                        Diagnostic::new(
                            Code::NullableAccess,
                            base.span,
                            format!("`{list}` may hold `nothing`, so it has no `{name}` to list."),
                        )
                        .with_help("go through it with `every … in … has …` and check each item `is not empty`"),
                    );
                    (ExprKind::Nothing, Type::Error)
                }
                Type::Error => (ExprKind::Nothing, Type::Error),
                _ => {
                    let diag = self.unknown_field(&self.type_name(&base_ty), field, []);
                    self.diags.push(diag);
                    (ExprKind::Nothing, Type::Error)
                }
            },
            _ => {
                let diag = self.unknown_field(&self.type_name(&base_ty), field, []);
                self.diags.push(diag);
                (ExprKind::Nothing, Type::Error)
            }
        }
    }

    /// The field called `field` of record type `id`, or `VL0205`.
    fn record_field(&mut self, id: TypeId, field: &ast::Ident) -> Option<(usize, Type)> {
        let record = self.file.types.get(id.0)?;
        if let Some((index, def)) = record.fields.iter().enumerate().find(|(_, f)| f.name == field.name) {
            return Some((index, def.ty.clone()));
        }
        let diag = self.unknown_field(&record.name, field, record.fields.iter().map(|f| f.name.as_str()));
        self.diags.push(diag);
        None
    }

    fn unary(&mut self, op: UnaryOp, operand: &'a ast::Expr) -> (ExprKind, Type) {
        let e = self.expr(operand, None);
        let (wanted, mut ty) = match op {
            UnaryOp::Neg => (Type::Number, Type::Number),
            UnaryOp::Not => (Type::Boolean, Type::Boolean),
        };
        match &e.ty {
            Type::Error => {}
            t if *t == wanted => {}
            Type::Optional(inner) if **inner == wanted && op == UnaryOp::Neg => self.nullable(operand),
            t => {
                let t = self.type_name(t);
                self.diags.push(Diagnostic::new(
                    Code::InvalidOperandType,
                    e.span,
                    format!("`{}` can't be used with {t}.", op.as_str()),
                ));
                if op == UnaryOp::Neg {
                    ty = Type::Error;
                }
            }
        }
        (
            ExprKind::Unary {
                op,
                operand: Box::new(e),
            },
            ty,
        )
    }

    fn binary(&mut self, op: BinaryOp, lhs: &'a ast::Expr, rhs: &'a ast::Expr) -> (ExprKind, Type) {
        let (l, r, ty) = match op {
            BinaryOp::And | BinaryOp::Or => {
                let l = self.expr(lhs, None);
                let r = self.narrowed(lhs, op == BinaryOp::And, |b| b.expr(rhs, None));
                if !(is(&l.ty, &Type::Boolean) && is(&r.ty, &Type::Boolean)) {
                    let diag = self
                        .invalid_operands(op, &l, &r)
                        .with_help(format!("both sides of `{}` must be true or false", op.as_str()));
                    self.diags.push(diag);
                }
                (l, r, Type::Boolean)
            }
            BinaryOp::Eq | BinaryOp::NotEq => {
                // `[]` takes its type from the other side, so that side is checked first.
                let (l, r) = if matches!(lhs.kind, Ast::List { .. }) && !matches!(rhs.kind, Ast::List { .. }) {
                    let r = self.expr_narrowed(rhs, None, false);
                    (self.expr_narrowed(lhs, Some(&r.ty), false), r)
                } else {
                    let l = self.expr_narrowed(lhs, None, false);
                    let r = self.expr_narrowed(rhs, Some(&l.ty), false);
                    (l, r)
                };
                if !assignable(&l.ty, &r.ty) && !assignable(&r.ty, &l.ty) {
                    let diag = self.invalid_operands(op, &l, &r);
                    self.diags.push(diag);
                }
                (l, r, Type::Boolean)
            }
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                let (l, r, _) = self.numeric(op, lhs, rhs);
                (l, r, Type::Boolean)
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                // A sum that already failed isn't a `Number` to compare again (R-CMP-10).
                let (l, r, ok) = self.numeric(op, lhs, rhs);
                (l, r, if ok { Type::Number } else { Type::Error })
            }
        };
        (
            ExprKind::Binary {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            },
            ty,
        )
    }

    /// Operands of an arithmetic or ordering operator: `Number`s (R-TYP-20 table, R-TYP-12). `false` if they aren't.
    fn numeric(&mut self, op: BinaryOp, lhs: &'a ast::Expr, rhs: &'a ast::Expr) -> (Expr, Expr, bool) {
        let l = self.expr(lhs, None);
        let r = self.expr(rhs, None);
        let optional_number = |t: &Type| matches!(t, Type::Optional(inner) if **inner == Type::Number);
        let fits = |t: &Type| is(t, &Type::Number) || optional_number(t);
        if !fits(&l.ty) || !fits(&r.ty) {
            let mut diag = self.invalid_operands(op, &l, &r);
            if l.ty == Type::Text && r.ty == Type::Text {
                diag = diag.with_help(match op {
                    BinaryOp::Add => "use `concat(a, b)` to join text",
                    _ => "text can only be compared with `==` and `!=`",
                });
            }
            self.diags.push(diag);
            return (l, r, false);
        }
        for (e, ast) in [(&l, lhs), (&r, rhs)] {
            if optional_number(&e.ty) {
                self.nullable(ast);
            }
        }
        (l, r, true)
    }

    /// `is empty` applies to `T?`, lists and text (R-TYP-20 table, R-TYP-21).
    fn check_emptiable(&mut self, operand: &Expr, negated: bool) {
        match &operand.ty {
            Type::Optional(_) | Type::List(_) | Type::Text | Type::Nothing | Type::Error => {}
            ty => {
                let name = self.type_name(ty);
                let op = if negated { "is not empty" } else { "is empty" };
                self.diags.push(
                    Diagnostic::new(
                        Code::InvalidOperandType,
                        operand.span,
                        format!("`{op}` can't be used with {name}."),
                    )
                    .with_help(format!("a `{name}` is never empty — did you mean `{name}?`")),
                );
            }
        }
    }

    /// `every x in xs has p` / `some x in xs has p`: `x` is a new name for each item of the list `xs`.
    fn quantified(
        &mut self,
        quantifier: ast::Quantifier,
        var: &'a ast::Ident,
        collection: &'a ast::Expr,
        body: &'a ast::Expr,
    ) -> (ExprKind, Type) {
        let collection_e = self.expr(collection, None);
        let element = match &collection_e.ty {
            Type::List(element) => (**element).clone(),
            Type::Error => Type::Error,
            Type::Optional(inner) if matches!(**inner, Type::List(_)) => {
                self.nullable(collection);
                Type::Error
            }
            ty => {
                let diag = self.mismatch(&collection_e, &["a list".to_owned()]);
                let word = match quantifier {
                    ast::Quantifier::Every => "every",
                    ast::Quantifier::Some => "some",
                };
                self.diags.push(diag.with_help(format!(
                    "`{word}` goes through the items of a list, not a {}",
                    self.type_name(ty)
                )));
                Type::Error
            }
        };
        if self.lookup(&var.name).is_some() {
            self.diags.push(
                Diagnostic::new(
                    Code::DuplicateBinding,
                    var.span,
                    format!("`{}` is already used in this goal.", var.name),
                )
                .with_help("give the item its own name"),
            );
        }
        self.vars.push((&var.name, element));
        let body_e = self.expr(body, None);
        self.vars.pop();
        self.expect_boolean(&body_e, "what follows `has` must be true or false");
        (
            ExprKind::Quantified {
                quantifier,
                var: var.name.clone(),
                collection: Box::new(collection_e),
                body: Box::new(body_e),
            },
            Type::Boolean,
        )
    }

    /// Checks values after an error, so their own mistakes are still reported but nothing is expected of them.
    fn skip<V: Value + 'a>(&mut self, values: impl IntoIterator<Item = &'a V>) {
        for value in values {
            value.check(self, Some(&Type::Error));
        }
    }

    fn expect_boolean(&mut self, e: &Expr, help: &str) {
        if !is(&e.ty, &Type::Boolean) {
            let diag = self.mismatch(e, &["Boolean".to_owned()]).with_help(help);
            self.diags.push(diag);
        }
    }

    /// `VL0204` for `e`, which should have been one of `wanted`.
    fn mismatch(&self, e: &Expr, wanted: &[String]) -> Diagnostic {
        let diag = Diagnostic::new(
            Code::TypeMismatch,
            e.span,
            format!("Expected {}, but got {}.", or_list(wanted), self.type_name(&e.ty)),
        );
        if matches!(e.ty, Type::Optional(_)) {
            let source = self.source(e.span);
            return diag.with_help(format!("it might be empty — check `{source} is not empty` first"));
        }
        diag
    }

    fn invalid_operands(&self, op: BinaryOp, l: &Expr, r: &Expr) -> Diagnostic {
        Diagnostic::new(
            Code::InvalidOperandType,
            l.span.to(r.span),
            format!(
                "`{}` can't be used with {} and {}.",
                op.as_str(),
                self.type_name(&l.ty),
                self.type_name(&r.ty)
            ),
        )
    }

    /// `VL0207` for the path or value `e` of type `T?` (R-TYP-12).
    fn nullable(&mut self, e: &ast::Expr) {
        let source = self.source(e.span);
        self.diags.push(
            Diagnostic::new(
                Code::NullableAccess,
                e.span,
                format!("`{source}` might be empty — check `is not empty` first."),
            )
            .with_help(format!("write `{source} is not empty and …` before using it")),
        );
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        let visible = self
            .vars
            .iter()
            .map(|(n, _)| *n)
            .chain(self.goal.params.iter().map(|p| p.name.as_str()))
            .chain(self.bindings.iter().map(|(n, _)| *n))
            .chain(["result"]);
        let help = match closest(name, visible) {
            Some(suggestion) => format!("did you mean `{suggestion}`?"),
            None => "a check can use the goal's inputs, its `call:` names and `result`".to_owned(),
        };
        self.diags.push(
            Diagnostic::new(Code::UnknownName, span, format!("I don't know what `{name}` is here.")).with_help(help),
        );
    }

    fn unknown_field<'n>(
        &self,
        owner: &str,
        field: &ast::Ident,
        fields: impl IntoIterator<Item = &'n str>,
    ) -> Diagnostic {
        let diag = Diagnostic::new(
            Code::UnknownField,
            field.span,
            format!("A `{owner}` doesn't have a field called `{}`.", field.name),
        );
        match closest(&field.name, fields) {
            Some(suggestion) => diag.with_help(format!("did you mean `{suggestion}`?")),
            None => diag,
        }
    }

    fn unknown_type(&self, name: &ast::Ident) -> Diagnostic {
        let diag = Diagnostic::new(
            Code::UnknownType,
            name.span,
            format!("I don't know a type called `{}`.", name.name),
        );
        match closest(&name.name, self.type_names()) {
            Some(suggestion) => diag.with_help(format!("did you mean `{suggestion}`?")),
            None => diag.with_help("declare it with `type`"),
        }
    }

    fn field_names_help(&self, id: TypeId) -> String {
        let Some(record) = self.file.types.get(id.0) else {
            return String::new();
        };
        let fields: Vec<String> = record.fields.iter().map(|f| format!("{}: …", f.name)).collect();
        format!("write them like `{}({})`", record.name, fields.join(", "))
    }

    fn type_names(&self) -> Vec<&'f str> {
        let names: &'f BTreeMap<&'f str, Declared> = &self.file.scope.names;
        names
            .iter()
            .filter_map(|(n, d)| match d {
                Declared::Type(_) => Some(*n),
                Declared::Goal(_) => None,
            })
            .collect()
    }

    fn type_name(&self, ty: &Type) -> String {
        ty.display(self.file.types)
    }

    fn source(&self, span: Span) -> &'f str {
        self.file.text.get(span.start..span.end).unwrap_or_default()
    }
}

/// `VL0302` for a call to `name` with `found` inputs instead of `expected`.
fn arity(name: &str, expected: usize, found: usize, span: Span) -> Diagnostic {
    let inputs = if expected == 1 { "input" } else { "inputs" };
    Diagnostic::new(
        Code::CallArityMismatch,
        span,
        format!("`{name}` needs {expected} {inputs}, but got {found}."),
    )
}

/// `ty` is `wanted`, or already has an error.
fn is(ty: &Type, wanted: &Type) -> bool {
    *ty == Type::Error || ty == wanted
}

/// The same type, where a type with an error matches anything (R-CMP-10).
fn same(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Error, _) | (_, Type::Error) => true,
        (Type::Optional(a), Type::Optional(b)) | (Type::List(a), Type::List(b)) => same(a, b),
        _ => a == b,
    }
}

/// R-TYP-20: `from` is assignable to `to` iff they're the same, or `to` is `U?` and `from` is `U` or `Nothing`.
pub(crate) fn assignable(from: &Type, to: &Type) -> bool {
    same(from, to) || matches!(to, Type::Optional(inner) if *from == Type::Nothing || same(from, inner))
}

/// R-TYP-26: the least type both `a` and `b` are assignable to, if there is one.
fn join(a: &Type, b: &Type) -> Option<Type> {
    if assignable(a, b) {
        Some(b.clone())
    } else if assignable(b, a) {
        Some(a.clone())
    } else if *a == Type::Nothing {
        Some(Type::Optional(Box::new(b.clone())))
    } else if *b == Type::Nothing {
        Some(Type::Optional(Box::new(a.clone())))
    } else {
        None
    }
}

/// `a`, `a or b`, `a, b or c`.
fn or_list(items: &[String]) -> String {
    joined(items, "or")
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    joined(items, "and")
}

fn joined(items: &[String], word: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} {word} {last}", init.join(", ")),
    }
}

/// What `T` and `U` stand for in one call of a built-in.
#[derive(Default)]
struct Vars {
    t: Option<Type>,
    u: Option<Type>,
}

impl Vars {
    fn slot(&mut self, shape: &Shape) -> Option<&mut Option<Type>> {
        match shape {
            Shape::T => Some(&mut self.t),
            Shape::U => Some(&mut self.u),
            _ => None,
        }
    }

    /// `shape` as a type, if everything in it is known.
    fn instantiate(&self, shape: &Shape) -> Option<Type> {
        Some(match shape {
            Shape::Number => Type::Number,
            Shape::Text => Type::Text,
            Shape::Boolean => Type::Boolean,
            Shape::T => self.t.clone()?,
            Shape::U => self.u.clone()?,
            Shape::List(element) => Type::List(Box::new(self.instantiate(element)?)),
            Shape::Optional(inner) => Type::Optional(Box::new(self.instantiate(inner)?)),
            Shape::Lambda(..) => return None,
        })
    }

    /// Whether an argument of type `ty` fits `shape`, fixing `T` and `U` the first time they're seen.
    fn unify(&mut self, shape: &Shape, ty: &Type) -> bool {
        match (shape, ty) {
            (_, Type::Error) => true,
            (Shape::T | Shape::U, _) => match self.slot(shape) {
                Some(Some(bound)) => assignable(ty, bound),
                Some(slot) => {
                    *slot = Some(ty.clone());
                    true
                }
                None => false,
            },
            (Shape::Optional(_), Type::Nothing) => true,
            _ => self.exact(shape, ty),
        }
    }

    /// Whether `ty` is exactly `shape`: inside a list, types are invariant (R-TYP-13).
    fn exact(&mut self, shape: &Shape, ty: &Type) -> bool {
        match (shape, ty) {
            (_, Type::Error)
            | (Shape::Number, Type::Number)
            | (Shape::Text, Type::Text)
            | (Shape::Boolean, Type::Boolean) => true,
            (Shape::T | Shape::U, _) => match self.slot(shape) {
                Some(Some(bound)) => same(ty, bound),
                Some(slot) => {
                    *slot = Some(ty.clone());
                    true
                }
                None => false,
            },
            (Shape::List(s), Type::List(t)) | (Shape::Optional(s), Type::Optional(t)) => self.exact(s, t),
            _ => false,
        }
    }

    /// `shape` for a learner: its type if known, else what kind of value it is.
    fn describe(&self, shape: &Shape, types: &[RecordType]) -> String {
        if let Some(ty) = self.instantiate(shape) {
            return ty.display(types);
        }
        match shape {
            Shape::List(_) => "a list".to_owned(),
            Shape::Optional(_) => "a value that may be `nothing`".to_owned(),
            _ => "a value".to_owned(),
        }
    }
}
