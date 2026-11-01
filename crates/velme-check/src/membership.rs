//! Domain membership (`language/13` R-CHK-15, `runtime/30` R-RUN-26–28): the closed set or the complete `where`
//! condition of a domain, evaluated from validated HIR on the reference interpreter, and the deep walk over a value that
//! tests every domain it reaches. `velme-runtime` calls the walk around each body, whatever backend ran it (R-RUN-27);
//! source literals and generated values are tested one domain at a time, each on a fresh budget
//! ([`Membership::member`]).

use velme_builtins::{Value, equals};
use velme_diagnostics::{Code, Diagnostic};
use velme_interp::{Budget, Error, Evaluator, Failure, Limits, Spent};
use velme_sema::hir::{DomainId, DomainRule, DomainType, Goal, Literal, Program, RecordType, Type};

use crate::lower::{Lowered, VALUE_LOCAL, lower_condition};

/// The fuel debited before each type/value pair the membership walk visits (`runtime/30` R-RUN-04, R-RUN-27).
pub const DOMAIN_VISIT_FUEL: u64 = 1;

/// The domains of a checked program, ready to test values against: each closed set's members and each condition
/// lowered once, and which types reach a domain at all, so a walk skips what holds none (R-RUN-26).
#[derive(Debug)]
pub struct Membership {
    records: Vec<RecordType>,
    /// Whether record type `i` reaches a domain through its fields.
    reach: Vec<bool>,
    domains: Vec<Declared>,
}

#[derive(Debug)]
struct Declared {
    declared: DomainType,
    rule: Rule,
    /// The condition as written, for a report.
    condition: String,
}

#[derive(Debug)]
enum Rule {
    /// The members, made once: a scan builds no list (R-RUN-27).
    OneOf(Vec<Value>),
    Where(Lowered),
}

/// A value outside a domain, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outside {
    /// The domain.
    pub domain: String,
    /// The first failing path, depth first: `result`, `player.age`, `scores[2]`.
    pub path: String,
    /// Why, in `reference/90`'s words: the failed constraint, or the condition and its value error.
    pub reason: String,
    /// The value error the condition raised, which makes it a non-member (R-CHK-15).
    pub cause: Option<Code>,
}

/// Why a walk refused a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// It is not a member.
    Outside(Outside),
    /// The walk stopped: out of fuel or memory, the watchdog, a size limit, or a bug. The code is kept (R-CHK-08).
    Stopped(Failure),
}

impl Refused {
    /// The `VL0902` of an input refused at the requested goal's entry (R-RUN-28), at the parameter `param` of `goal`.
    pub fn input_diagnostic(&self, goal: &Goal, param: usize) -> Diagnostic {
        let span = goal.params.get(param).map_or(goal.span, |p| p.span);
        match self {
            Refused::Outside(outside) => outside.noted(Diagnostic::new(
                Code::InvalidInput,
                span,
                format!(
                    "Input `{}` is outside `{}`: {}.",
                    outside.path, outside.domain, outside.reason
                ),
            )),
            Refused::Stopped(failure) => failure.diagnostic(&goal.name, goal.span),
        }
    }

    /// The `VL0504` of an output refused before `goal`'s own checks (R-RUN-26).
    pub fn output_diagnostic(&self, goal: &Goal) -> Diagnostic {
        match self {
            Refused::Outside(outside) => outside.noted(Diagnostic::new(
                Code::OutputOutsideDomain,
                goal.span,
                format!(
                    "`{}` returned a value outside `{}` at `{}`: {}.",
                    goal.name, outside.domain, outside.path, outside.reason
                ),
            )),
            Refused::Stopped(failure) => failure.diagnostic(&goal.name, goal.span),
        }
    }
}

impl Outside {
    /// `diag` with the value error that caused it, if one did (`reference/90` R-ERR-04).
    pub(crate) fn noted(&self, diag: Diagnostic) -> Diagnostic {
        match self.cause {
            Some(code) => diag.with_note(format!("cause: {}", code.as_str())),
            None => diag,
        }
    }
}

/// One step of a path into a value.
enum Step<'v> {
    Field(&'v str),
    Index(usize),
}

/// How a walk pays: from an invocation's meter, or one predicate on a fresh budget of `fuel` at no visit cost, what it
/// spent recorded.
enum Meter<'e, 's> {
    Invocation(&'e mut Evaluator<'s>),
    Fresh { fuel: u64, spent: u64 },
}

impl<'s> Meter<'_, 's> {
    fn debit(&mut self, fuel: u64) -> Result<(), Refused> {
        match self {
            Meter::Invocation(evaluator) => evaluator.debit(fuel).map_err(Refused::Stopped),
            Meter::Fresh { .. } => Ok(()),
        }
    }

    fn eval(&mut self, condition: &'s Lowered, value: &Value) -> Result<Value, Failure> {
        let expr = condition.node.trusted();
        match self {
            Meter::Invocation(evaluator) => evaluator.eval_bound(VALUE_LOCAL, value.clone(), expr),
            Meter::Fresh { fuel, spent } => {
                let mut evaluator = Evaluator::new(Limits {
                    fuel: *fuel,
                    ..Limits::SYSTEM
                });
                let result = evaluator.eval_bound(VALUE_LOCAL, value.clone(), expr);
                *spent = spent.saturating_add(evaluator.spent().fuel);
                result
            }
        }
    }
}

fn internal() -> Refused {
    Refused::Stopped(Error::Builtin(velme_builtins::Error::Internal).into())
}

impl Membership {
    /// The domains of the checked `program`, whose source is `source`.
    pub fn new(program: &Program, source: &str) -> Result<Membership, Diagnostic> {
        let mut domains = Vec::with_capacity(program.domains.len());
        for (i, declared) in program.domains.iter().enumerate() {
            let (rule, condition) = match &declared.rule {
                DomainRule::OneOf { members } => (
                    Rule::OneOf(members.iter().map(|m| Value::text(m)).collect()),
                    String::new(),
                ),
                DomainRule::Where { condition } => {
                    let lowered = lower_condition(program, DomainId(i))?.ok_or_else(Diagnostic::internal_error)?;
                    let text = source
                        .get(condition.span.start..condition.span.end)
                        .unwrap_or("its condition");
                    (Rule::Where(lowered), text.to_owned())
                }
            };
            domains.push(Declared {
                declared: declared.clone(),
                rule,
                condition,
            });
        }
        let mut membership = Membership {
            records: program.types.clone(),
            reach: Vec::new(),
            domains,
        };
        // Records are never recursive (R-TYP-18) and a field's record may come later, so a pass per record settles them.
        membership.reach = vec![false; program.types.len()];
        for _ in 0..=program.types.len() {
            let reach: Vec<bool> = membership
                .records
                .iter()
                .map(|r| r.fields.iter().any(|f| membership.reaches(&f.ty)))
                .collect();
            if reach == membership.reach {
                break;
            }
            membership.reach = reach;
        }
        Ok(membership)
    }

    /// Whether a value of type `ty` can hold a domain value anywhere: a walk visits nothing in a type that can't.
    pub fn reaches(&self, ty: &Type) -> bool {
        match ty {
            Type::Domain(..) => true,
            Type::Optional(inner) | Type::List(inner) => self.reaches(inner),
            Type::Record(id) => self.reach.get(id.0).copied().unwrap_or(false),
            _ => false,
        }
    }

    /// Walks `goal`'s `inputs`, one per parameter, at the requested goal's entry (R-RUN-28), within `budget`: the
    /// first refused input's index and why, and what the invocation has spent by then either way.
    pub fn inputs(&self, goal: &Goal, inputs: &[Value], budget: Budget) -> (Result<(), (usize, Refused)>, Spent) {
        let mut evaluator = Evaluator::from_budget(budget);
        if inputs.len() != goal.params.len() {
            return (Err((0, internal())), evaluator.spent());
        }
        for (i, (param, value)) in goal.params.iter().zip(inputs).enumerate() {
            let mut meter = Meter::Invocation(&mut evaluator);
            if let Err(refused) = self.walk(&param.ty, value, &param.name, &mut meter) {
                return (Err((i, refused)), evaluator.spent());
            }
        }
        (Ok(()), evaluator.spent())
    }

    /// Walks `goal`'s `output` before its own checks (R-RUN-26), within `budget`, which has spent what the body left
    /// (R-RUN-27): why it was refused if it was, and what the invocation has spent by then either way.
    pub fn output(&self, goal: &Goal, output: &Value, budget: Budget) -> (Result<(), Refused>, Spent) {
        let mut evaluator = Evaluator::from_budget(budget);
        let mut meter = Meter::Invocation(&mut evaluator);
        let walked = self.walk(&goal.output, output, "result", &mut meter);
        (walked, evaluator.spent())
    }

    /// Whether `value` is a member of the domain `id` alone, on a fresh system budget (R-CHK-15): the predicate the
    /// source-literal pass supplies each constant to (`language/11` R-TYP-31), and `velme-synth`'s generator and probe
    /// each value they make (`compiler/22` R-SYNTH-63, R-SYNTH-65), the walk not being one.
    pub fn member(&self, id: DomainId, value: &Value) -> Result<(), Refused> {
        self.member_within(id, value, Limits::SYSTEM.fuel).0
    }

    /// [`Membership::member`] on a fresh system budget whose fuel is at most `fuel`, and the fuel it spent: what
    /// `velme-synth` totals across one domain's probe or one goal's generation (`compiler/22` R-SYNTH-67, D-167).
    pub fn member_within(&self, id: DomainId, value: &Value, fuel: u64) -> (Result<(), Refused>, u64) {
        let mut meter = Meter::Fresh {
            fuel: fuel.min(Limits::SYSTEM.fuel),
            spent: 0,
        };
        let result = self.test(id, value, &mut meter);
        let Meter::Fresh { spent, .. } = meter else {
            return (result, 0);
        };
        (result, spent)
    }

    /// The domain `id`'s name.
    pub fn name(&self, id: DomainId) -> Option<&str> {
        self.domains.get(id.0).map(|d| d.declared.name.as_str())
    }

    fn walk<'s>(&'s self, ty: &Type, value: &Value, root: &str, meter: &mut Meter<'_, 's>) -> Result<(), Refused> {
        if !self.reaches(ty) {
            return Ok(());
        }
        let mut path = Vec::new();
        self.visit(ty, value, &mut path, meter)
            .map_err(|refused| match refused {
                Refused::Outside(mut outside) => {
                    outside.path = format!("{root}{}", outside.path);
                    Refused::Outside(outside)
                }
                stopped => stopped,
            })
    }

    /// One visited type/value pair, debited first (R-RUN-27): at a domain, its membership and then its base; inside
    /// a list, record or present optional, each part that reaches a domain, in declaration and index order.
    fn visit<'v, 's>(
        &'s self,
        ty: &Type,
        value: &'v Value,
        path: &mut Vec<Step<'v>>,
        meter: &mut Meter<'_, 's>,
    ) -> Result<(), Refused> {
        meter.debit(DOMAIN_VISIT_FUEL)?;
        match ty {
            Type::Domain(id, base) => {
                self.test(*id, value, meter).map_err(|refused| match refused {
                    Refused::Outside(mut outside) => {
                        outside.path = shown(path);
                        Refused::Outside(outside)
                    }
                    stopped => stopped,
                })?;
                // A domain's base is always visited, a scalar's too.
                self.visit(base, value, path, meter)
            }
            Type::Optional(inner) => match value {
                Value::Nothing => Ok(()),
                present => self.part(inner, present, path, meter),
            },
            Type::List(element) => {
                let Value::List(items) = value else {
                    return Err(internal());
                };
                for (i, item) in items.iter().enumerate() {
                    path.push(Step::Index(i));
                    self.part(element, item, path, meter)?;
                    path.pop();
                }
                Ok(())
            }
            Type::Record(id) => {
                let (Value::Record(record), Some(declared)) = (value, self.records.get(id.0)) else {
                    return Err(internal());
                };
                if record.fields.len() != declared.fields.len() {
                    return Err(internal());
                }
                for (def, (name, field)) in declared.fields.iter().zip(&record.fields) {
                    path.push(Step::Field(name));
                    self.part(&def.ty, field, path, meter)?;
                    path.pop();
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// A part of a visited value, visited only if its type reaches a domain (R-RUN-26).
    fn part<'v, 's>(
        &'s self,
        ty: &Type,
        value: &'v Value,
        path: &mut Vec<Step<'v>>,
        meter: &mut Meter<'_, 's>,
    ) -> Result<(), Refused> {
        if self.reaches(ty) {
            self.visit(ty, value, path, meter)
        } else {
            Ok(())
        }
    }

    /// One predicate: the closed set scanned as `contains` would be, without a list, or the whole condition once
    /// (R-RUN-27). The path of an [`Outside`] is left to the caller.
    fn test<'s>(&'s self, id: DomainId, value: &Value, meter: &mut Meter<'_, 's>) -> Result<(), Refused> {
        let domain = self.domains.get(id.0).ok_or_else(internal)?;
        let outside = |reason: String, cause: Option<Code>| {
            Refused::Outside(Outside {
                domain: domain.declared.name.clone(),
                path: String::new(),
                reason,
                cause,
            })
        };
        match &domain.rule {
            Rule::OneOf(members) => {
                if !matches!(value, Value::Text(_)) {
                    return Err(internal());
                }
                // `contains`'s catalog cost: its unit, then per member scanned its unit and the comparison's.
                meter.debit(1)?;
                for member in members {
                    let compared = equals(member, value, u64::MAX).map_err(|_| internal())?;
                    meter.debit(1u64.saturating_add(compared.fuel))?;
                    if compared.value == Value::Boolean(true) {
                        return Ok(());
                    }
                }
                Err(outside("it isn't one of its members".to_owned(), None))
            }
            Rule::Where(condition) => match meter.eval(condition, value) {
                Ok(Value::Boolean(true)) => Ok(()),
                Ok(Value::Boolean(false)) => Err(outside(domain.reason(value), None)),
                Ok(_) => Err(internal()),
                Err(failure) => match failure.error {
                    Error::Builtin(velme_builtins::Error::Arithmetic { op }) => Err(outside(
                        format!("`{}` tried to {op}, which has no answer", domain.condition),
                        Some(Code::ArithmeticError),
                    )),
                    _ => Err(Refused::Stopped(failure)),
                },
            },
        }
    }
}

impl Declared {
    /// Why `value`, for which the condition is false, isn't a member: the simple constraint it breaks if it breaks one,
    /// in the static check's words (`language/11` R-TYP-31), else the condition.
    fn reason(&self, value: &Value) -> String {
        let simple = match value {
            Value::Number(n) => self.declared.literal_violation(&Literal::Number(*n)),
            Value::Text(t) => self.declared.literal_violation(&Literal::Text(t)),
            _ => None,
        };
        simple.map_or_else(|| format!("`{}` is false", self.condition), |(_, reason)| reason)
    }
}

/// A path's steps after its root: `.age`, `[2]`.
fn shown(path: &[Step<'_>]) -> String {
    path.iter()
        .map(|step| match step {
            Step::Field(name) => format!(".{name}"),
            Step::Index(i) => format!("[{i}]"),
        })
        .collect()
}
