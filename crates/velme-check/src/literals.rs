//! Full membership of source literals in domain positions (`language/11` R-TYP-31): after the static pass of
//! `velme-sema`, which knows only closed sets and simple constraints, each constant a call argument or an example puts
//! where a domain is expected — at the top or nested in its records, lists and present optionals — is tested against
//! the domain's complete condition, each on a fresh system budget. A literal of a check builds an underlying record,
//! never a domain value, so checks are not held to it (R-TYP-28).

use velme_diagnostics::{Code, Diagnostic};
use velme_interp::{Evaluator, Limits};
use velme_ir::CheckScope;
use velme_sema::hir::{DomainId, Expr, ExprKind, Goal, Program, Type, UnaryOp};

use crate::lower::lower_literal;
use crate::membership::{Membership, Refused};

/// The `VL0204` of every source literal of `program`, whose source is `source`, outside the domain its position expects,
/// in source order; a budget or internal failure keeps its code. `program` has no errors.
pub fn check_literals(program: &Program, source: &str) -> Vec<Diagnostic> {
    if program.domains.is_empty() {
        return Vec::new();
    }
    let membership = match Membership::new(program, source) {
        Ok(membership) => membership,
        Err(diagnostic) => return vec![diagnostic],
    };
    let mut found = Vec::new();
    for goal in &program.goals {
        let scope = match CheckScope::new(program, goal) {
            Ok(scope) => scope,
            Err(diagnostic) => return vec![diagnostic],
        };
        let pass = Pass {
            program,
            source,
            goal,
            scope: &scope,
            membership: &membership,
        };
        for binding in &goal.bindings {
            let Some(callee) = program.goals.get(binding.callee.0) else {
                return vec![Diagnostic::internal_error()];
            };
            for (arg, param) in binding.args.iter().zip(&callee.params) {
                pass.position(arg, &param.ty, &mut found);
            }
        }
        for example in &goal.examples {
            for (arg, param) in example.args.iter().zip(&goal.params) {
                pass.position(arg, &param.ty, &mut found);
            }
            pass.position(&example.expected, &goal.output, &mut found);
        }
    }
    velme_diagnostics::sort(&mut found);
    found
}

struct Pass<'a, 'p> {
    program: &'p Program,
    source: &'p str,
    goal: &'p Goal,
    scope: &'a CheckScope<'p>,
    membership: &'a Membership,
}

impl Pass<'_, '_> {
    /// `e` where a `ty` is expected: tested at a domain, then through its base; inside a record, list or present
    /// optional literal, each part at its own type. Anything but a constant is left alone: a path's value is a member
    /// by its producer's output check (R-RUN-26).
    fn position(&self, e: &Expr, ty: &Type, found: &mut Vec<Diagnostic>) {
        if !self.membership.reaches(ty) || !constant(e) {
            return;
        }
        match ty {
            Type::Domain(id, base) => {
                if let Some(diagnostic) = self.test(e, ty, *id) {
                    found.push(diagnostic);
                    return;
                }
                self.position(e, base, found);
            }
            Type::Optional(inner) if !matches!(e.kind, ExprKind::Nothing) => self.position(e, inner, found),
            Type::List(element) => {
                if let ExprKind::List { items } = &e.kind {
                    for item in items {
                        self.position(item, element, found);
                    }
                }
            }
            Type::Record(id) => {
                if let (ExprKind::Record { fields, .. }, Some(record)) = (&e.kind, self.program.record(*id)) {
                    for (def, field) in record.fields.iter().zip(fields) {
                        self.position(field, &def.ty, found);
                    }
                }
            }
            _ => {}
        }
    }

    /// The diagnostic of `e`, a constant at the domain `ty` (`id`), if it isn't a member.
    fn test(&self, e: &Expr, ty: &Type, id: DomainId) -> Option<Diagnostic> {
        let value = match lower_literal(self.program, self.goal, self.scope, e, ty) {
            Ok(lowered) => match Evaluator::new(Limits::SYSTEM).eval(lowered.node.trusted()) {
                Ok(value) => value,
                Err(failure) => return Some(failure.diagnostic(&self.goal.name, e.span)),
            },
            Err(diagnostic) => return Some(diagnostic),
        };
        match self.membership.member(id, &value) {
            Ok(()) => None,
            Err(Refused::Outside(outside)) => Some(
                outside.noted(
                    Diagnostic::new(
                        Code::TypeMismatch,
                        e.span,
                        format!(
                            "`{}` isn't a valid `{}`: {}.",
                            self.source.get(e.span.start..e.span.end).unwrap_or("?"),
                            outside.domain,
                            outside.reason
                        ),
                    )
                    .with_help(format!("use a value `{}` allows", outside.domain)),
                ),
            ),
            Err(Refused::Stopped(failure)) => Some(failure.diagnostic(&self.goal.name, e.span)),
        }
    }
}

/// Whether `e` is a constant of `language/10` §4's literal grammar, nested records and lists included.
fn constant(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Number { .. } | ExprKind::Text { .. } | ExprKind::Bool { .. } | ExprKind::Nothing => true,
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => constant(operand),
        ExprKind::List { items } => items.iter().all(constant),
        ExprKind::Record { fields, .. } => fields.iter().all(constant),
        _ => false,
    }
}
