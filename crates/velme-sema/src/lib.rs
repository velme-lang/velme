//! Velme semantic analysis: names, types and the call graph, producing the typed HIR (`compiler/20` §3 phases 3–6).
#![forbid(unsafe_code)]

mod budget;
mod check;
mod graph;
pub mod hir;
mod resolve;

use velme_diagnostics::Diagnostic;
use velme_syntax::LANGUAGE_VERSION;
/// The input of [`analyze`], so its callers need not depend on `velme-syntax`.
pub use velme_syntax::SourceFile;

/// Parses and checks a whole file (`compiler/20` §6): phases 1–6, never touching the network, the store or the clock
/// (R-CMP-05). The program is returned only when there is no error; warnings don't count (D-69). Diagnostics are
/// sorted by position, then code (R-CMP-16).
pub fn analyze(file: &SourceFile) -> (Option<hir::Program>, Vec<Diagnostic>) {
    analyze_with(file, hir::Budget::SYSTEM)
}

/// [`analyze`] where a goal's `budget` line lowers `defaults` rather than the system caps: a project's `[budget]`
/// (`tooling/40` §5.1, D-8). A key the line sets is its own; `defaults` is clamped to the system caps here, so a caller can
/// lower the caps and never raise them.
pub fn analyze_with(file: &SourceFile, defaults: hir::Budget) -> (Option<hir::Program>, Vec<Diagnostic>) {
    let (ast, diags) = velme_syntax::parse(file);
    analyze_parsed(file, &ast, diags, defaults)
}

/// [`analyze_with`] of `file` already parsed into `ast`, with the parser's `diags`: phases 3–6 alone, so a caller can
/// time parsing and checking apart (`tooling/40` §2, D-137).
pub fn analyze_parsed(
    file: &SourceFile,
    ast: &velme_syntax::ast::Program,
    mut diags: Vec<Diagnostic>,
    defaults: hir::Budget,
) -> (Option<hir::Program>, Vec<Diagnostic>) {
    let system = hir::Budget::SYSTEM;
    let defaults = hir::Budget {
        max_fuel: defaults.max_fuel.min(system.max_fuel),
        max_memory: defaults.max_memory.min(system.max_memory),
        max_goal_calls: defaults.max_goal_calls.min(system.max_goal_calls),
        max_call_depth: defaults.max_call_depth.min(system.max_call_depth),
    };
    let (types, mut goals, scope) = resolve::resolve(ast, &file.text, &mut diags);
    check::check_bodies(&types, &mut goals, &scope, &file.text, defaults, &mut diags);
    graph::check_graph(&goals, &scope.goals, &mut diags);
    velme_diagnostics::sort(&mut diags);
    let program = hir::Program {
        language_version: ast
            .header
            .as_ref()
            .map_or_else(|| LANGUAGE_VERSION.to_owned(), |h| h.version.text.clone()),
        types,
        goals,
    };
    let ok = !diags.iter().any(Diagnostic::is_error);
    (ok.then_some(program), diags)
}
