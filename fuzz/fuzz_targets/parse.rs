//! The parser and both renderers are total: any bytes end in an AST or diagnostics, never a panic (R-SYN-19,
//! AC-SYN-13). Run with `cargo +nightly fuzz run parse`; `fuzz/corpus/parse/` is replayed by `cargo test` on stable.
#![no_main]

use libfuzzer_sys::fuzz_target;
use velme_diagnostics::render::{self, JsonDiagnostic, LineIndex};

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = velme_syntax::SourceFile::from_bytes("fuzz.velme", data.to_vec()) {
        let (_, diags) = velme_syntax::parse(&file);
        let _ = render::render_human(&diags, &file.path, Some(&file.text), false);
        let lines = LineIndex::new(&file.text);
        let _: Vec<_> = diags.iter().map(|d| JsonDiagnostic::new(d, &file.path, &lines)).collect();
    }
});
