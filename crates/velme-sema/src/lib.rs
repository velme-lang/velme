//! Velme semantic analysis: names, types and the call graph, producing the typed HIR (`compiler/20` §3 phases 3–6).
#![forbid(unsafe_code)]

pub mod hir;
mod resolve;

use velme_diagnostics::Diagnostic;
use velme_syntax::{LANGUAGE_VERSION, SourceFile};

/// Parses and checks a whole file (`compiler/20` §6): phases 1–6, never touching the network, the store or the clock
/// (R-CMP-05). The program is returned only when there is no error; warnings don't count (D-69). Diagnostics are
/// sorted by position, then code (R-CMP-16).
pub fn analyze(file: &SourceFile) -> (Option<hir::Program>, Vec<Diagnostic>) {
    let (ast, mut diags) = velme_syntax::parse(file);
    let (types, goals, _scope) = resolve::resolve(&ast, &file.text, &mut diags);
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
