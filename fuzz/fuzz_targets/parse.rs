//! The parser is total: any bytes end in an AST or diagnostics, never a panic (R-SYN-19, AC-SYN-13).
//! Run with `cargo +nightly fuzz run parse`; `fuzz/corpus/parse/` is replayed by `cargo test` on stable.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(file) = velme_syntax::SourceFile::from_bytes("fuzz.velme", data.to_vec()) {
        let _ = velme_syntax::parse(&file);
    }
});
