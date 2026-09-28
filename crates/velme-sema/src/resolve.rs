//! Phase 3, resolve (`compiler/20` §3): the declaration namespace, record types and goal signatures
//! (`language/11` §7, `language/12` R-GOAL-02).

use std::collections::{BTreeMap, BTreeSet};

use velme_diagnostics::render::LineIndex;
use velme_diagnostics::{Code, Diagnostic, Span, closest};
use velme_syntax::BuiltinType;
use velme_syntax::ast::{self, BaseType, Decl, TypeExpr};

use crate::graph::shortest_cycle;
use crate::hir::{Budget, FieldDef, Goal, GoalId, GoalKind, Param, RecordType, Type, TypeId};

/// What a declared name refers to.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Declared {
    Type(TypeId),
    Goal(GoalId),
}

/// The file's declarations after resolving, for the later phases.
pub(crate) struct Scope<'a> {
    /// Type and goal names (R-TYP-19); a name declared twice keeps its first declaration.
    pub names: BTreeMap<&'a str, Declared>,
    /// Names of declarations that failed to parse or hold an error: references to them are not reported again
    /// (R-SYN-17).
    pub failed: BTreeSet<&'a str>,
    /// Each goal's declaration, by [`GoalId`].
    pub goals: Vec<&'a ast::GoalDecl>,
    /// Goals whose name is taken, with their signatures: their bodies are checked too (CC-ERR-04).
    pub unnamed_goals: Vec<(&'a ast::GoalDecl, Goal)>,
}

impl Scope<'_> {
    /// Whether `name` belongs to a declaration that already has an error.
    pub fn is_failed(&self, name: &str) -> bool {
        self.failed.contains(name)
    }
}

/// Resolves the declarations of `program`. A type or goal declared twice keeps its first declaration; the other is
/// still checked, so its own mistakes are reported too (CC-ERR-04).
///
/// D-64 needs no lint here: built-in function names are snake_case, so a type or goal named like one already gets the
/// naming warning, whose suggested name doesn't collide (R-SYN-06).
pub(crate) fn resolve<'a>(
    program: &'a ast::Program,
    text: &str,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<RecordType>, Vec<Goal>, Scope<'a>) {
    let lines = LineIndex::new(text);
    let mut scope = Scope {
        names: BTreeMap::new(),
        failed: program
            .failed
            .iter()
            .filter_map(|d| d.name.as_ref().map(|n| n.name.as_str()))
            .collect(),
        goals: Vec::new(),
        unnamed_goals: Vec::new(),
    };
    let mut type_decls = Vec::new();
    // Declarations whose name is taken: checked, never referenced.
    let mut unnamed: Vec<&Decl> = Vec::new();
    let mut first_span: BTreeMap<&str, Span> = BTreeMap::new();
    for decl in &program.decls {
        let name = match decl {
            Decl::Type(t) => &t.name,
            Decl::Goal(g) => &g.name,
        };
        if let Some(builtin) = BuiltinType::from_name(&name.name) {
            diags.push(
                Diagnostic::new(
                    Code::DuplicateDeclaration,
                    name.span,
                    format!("`{}` is already the name of a built-in type.", builtin.as_str()),
                )
                .with_help("choose a different name"),
            );
            // A goal of this name is still called elsewhere; those calls aren't reported again.
            scope.failed.insert(&name.name);
            unnamed.push(decl);
            continue;
        }
        if let Some(&first) = first_span.get(name.name.as_str()) {
            diags.push(already_defined(&name.name, name.span, first, &lines));
            unnamed.push(decl);
            continue;
        }
        first_span.insert(&name.name, name.span);
        match decl {
            Decl::Type(t) => {
                scope.names.insert(&name.name, Declared::Type(TypeId(type_decls.len())));
                type_decls.push(t);
            }
            Decl::Goal(g) => {
                scope
                    .names
                    .insert(&name.name, Declared::Goal(GoalId(scope.goals.len())));
                scope.goals.push(g);
            }
        }
    }

    let types: Vec<RecordType> = type_decls
        .iter()
        .map(|t| record_type(t, &scope, &lines, diags))
        .collect();
    report_recursive_types(&types, &type_decls, diags);
    let goals = scope
        .goals
        .iter()
        .map(|g| goal_signature(g, &scope, &lines, diags))
        .collect();
    for decl in unnamed {
        match decl {
            Decl::Type(t) => drop(record_type(t, &scope, &lines, diags)),
            Decl::Goal(g) => {
                let goal = goal_signature(g, &scope, &lines, diags);
                scope.unnamed_goals.push((g, goal));
            }
        }
    }
    (types, goals, scope)
}

/// A record type with its fields resolved; a field declared twice is reported and left out (R-TYP-16).
fn record_type(
    decl: &ast::TypeDecl,
    scope: &Scope<'_>,
    lines: &LineIndex<'_>,
    diags: &mut Vec<Diagnostic>,
) -> RecordType {
    let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
    let mut fields = Vec::new();
    for field in &decl.fields {
        if let Some(&first) = seen.get(field.name.name.as_str()) {
            diags.push(already_defined(&field.name.name, field.name.span, first, lines));
            continue;
        }
        seen.insert(&field.name.name, field.name.span);
        fields.push(FieldDef {
            name: field.name.name.clone(),
            ty: declared_type(&field.ty, scope, diags),
            span: field.span,
        });
    }
    RecordType {
        name: decl.name.name.clone(),
        fields,
        span: decl.span,
    }
}

/// A goal's signature. A parameter declared twice is reported but kept, so the goal's input count stays as written
/// and calls to it aren't reported again (R-GOAL-02).
fn goal_signature(decl: &ast::GoalDecl, scope: &Scope<'_>, lines: &LineIndex<'_>, diags: &mut Vec<Diagnostic>) -> Goal {
    let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
    let params = decl
        .params
        .iter()
        .map(|param| {
            if let Some(&first) = seen.get(param.name.name.as_str()) {
                diags.push(already_defined(&param.name.name, param.name.span, first, lines));
            } else {
                seen.insert(&param.name.name, param.name.span);
            }
            Param {
                name: param.name.name.clone(),
                ty: declared_type(&param.ty, scope, diags),
                span: param.span,
            }
        })
        .collect();
    Goal {
        name: decl.name.name.clone(),
        params,
        output: declared_type(&decl.output, scope, diags),
        kind: GoalKind::Leaf,
        budget: Budget::SYSTEM,
        bindings: Vec::new(),
        plan: decl.plan.as_ref().map(|p| p.text.clone()),
        checks: Vec::new(),
        examples: Vec::new(),
        span: decl.span,
    }
}

/// `VL0203` for `name` at `span`, first declared at `first`.
fn already_defined(name: &str, span: Span, first: Span, lines: &LineIndex<'_>) -> Diagnostic {
    let line = lines.locate(first).line;
    Diagnostic::new(
        Code::DuplicateDeclaration,
        span,
        format!("`{name}` is already defined on line {line}."),
    )
    .with_help("rename one of them")
}

/// The type of a parameter, field or output: [`resolve_type`], and never `Nothing` or `Nothing?` (R-TYP-03).
fn declared_type(expr: &TypeExpr, scope: &Scope<'_>, diags: &mut Vec<Diagnostic>) -> Type {
    let ty = resolve_type(expr, scope, diags);
    let only_nothing = match &ty {
        Type::Nothing => true,
        Type::Optional(inner) => **inner == Type::Nothing,
        _ => false,
    };
    if !only_nothing {
        return ty;
    }
    diags.push(
        Diagnostic::new(
            Code::TypeMismatch,
            expr.span,
            "A value that can only be `nothing` carries no information.",
        )
        .with_help("use the type of the value, with `?` if it may be missing, like `Number?`"),
    );
    Type::Error
}

/// Resolves a written type (`language/11` §2): `VL0201` for a name that isn't a type.
pub(crate) fn resolve_type(expr: &TypeExpr, scope: &Scope<'_>, diags: &mut Vec<Diagnostic>) -> Type {
    let base = match &expr.base {
        BaseType::List { element } => Type::List(Box::new(resolve_type(element, scope, diags))),
        BaseType::Named { name } => named_type(name, scope, diags),
    };
    match base {
        Type::Error => Type::Error,
        base if expr.optional => Type::Optional(Box::new(base)),
        base => base,
    }
}

fn named_type(name: &ast::Ident, scope: &Scope<'_>, diags: &mut Vec<Diagnostic>) -> Type {
    match BuiltinType::from_name(&name.name) {
        Some(BuiltinType::Number) => return Type::Number,
        Some(BuiltinType::Text) => return Type::Text,
        Some(BuiltinType::Boolean) => return Type::Boolean,
        Some(BuiltinType::Nothing) => return Type::Nothing,
        Some(BuiltinType::List) => {
            diags.push(
                Diagnostic::new(Code::UnknownType, name.span, "`List` needs to say what it holds.")
                    .with_help("write the type of its items in `< >`, like `List<Number>`"),
            );
            return Type::Error;
        }
        None => {}
    }
    match scope.names.get(name.name.as_str()) {
        Some(Declared::Type(id)) => Type::Record(*id),
        Some(Declared::Goal(_)) => {
            diags.push(
                Diagnostic::new(
                    Code::UnknownType,
                    name.span,
                    format!("`{}` is a goal, not a type.", name.name),
                )
                .with_help("use a type here, like `Number` or a `type` declared in this file"),
            );
            Type::Error
        }
        None if scope.is_failed(&name.name) => Type::Error,
        None => {
            let types = BuiltinType::ALL
                .iter()
                .map(|t| t.as_str())
                .chain(scope.names.iter().filter_map(|(n, d)| match d {
                    Declared::Type(_) => Some(*n),
                    Declared::Goal(_) => None,
                }));
            let mut diag = Diagnostic::new(
                Code::UnknownType,
                name.span,
                format!("I don't know a type called `{}`.", name.name),
            );
            if let Some(suggestion) = closest(&name.name, types) {
                diag = diag.with_help(format!("did you mean `{suggestion}`?"));
            } else {
                diag = diag.with_help("declare it with `type`, or use `Number`, `Text` or `Boolean`");
            }
            diags.push(diag);
            Type::Error
        }
    }
}

/// The record types `ty` mentions directly, through `List` and `?`.
fn mentioned_records(ty: &Type, out: &mut Vec<TypeId>) {
    match ty {
        Type::Record(id) => out.push(*id),
        Type::Optional(inner) | Type::List(inner) => mentioned_records(inner, out),
        Type::Number | Type::Text | Type::Boolean | Type::Nothing | Type::Error => {}
    }
}

/// R-TYP-18: `VL0208` once per cycle of record types, at its first type in source order, with the path.
fn report_recursive_types(types: &[RecordType], decls: &[&ast::TypeDecl], diags: &mut Vec<Diagnostic>) {
    // For each type, the types its fields mention, in field order, with the field.
    let edges: Vec<Vec<(TypeId, usize)>> = types
        .iter()
        .map(|t| {
            let mut out = Vec::new();
            for (i, field) in t.fields.iter().enumerate() {
                let mut ids = Vec::new();
                mentioned_records(&field.ty, &mut ids);
                out.extend(ids.into_iter().map(|id| (id, i)));
            }
            out
        })
        .collect();
    let mut reported: BTreeSet<TypeId> = BTreeSet::new();
    for start in (0..types.len()).map(TypeId) {
        if reported.contains(&start) {
            continue;
        }
        let Some(cycle) = shortest_cycle(start, |t: TypeId| edges.get(t.0).map_or(&[][..], Vec::as_slice)) else {
            continue;
        };
        reported.extend(cycle.iter().map(|&(id, _)| id));
        let name = |id: TypeId| types.get(id.0).map_or("?", |t| t.name.as_str());
        let path: Vec<&str> = cycle
            .iter()
            .map(|&(id, _)| name(id))
            .chain(std::iter::once(name(start)))
            .collect();
        let (Some(decl), Some(&(_, field))) = (decls.get(start.0), cycle.first()) else {
            continue;
        };
        let mut diag = Diagnostic::new(
            Code::RecursiveType,
            decl.name.span,
            format!("`{}` contains itself, which Velme doesn't allow yet.", decl.name.name),
        )
        .with_note(path.join(" → "));
        if let Some(field) = decl.fields.get(field) {
            diag = diag.with_label(field.span, "this field leads back to it");
        }
        diags.push(diag.with_help("store the related values in a separate list instead of inside each other"));
    }
}
