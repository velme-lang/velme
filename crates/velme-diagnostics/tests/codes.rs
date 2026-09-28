//! The code enum matches `reference/90` §2 exactly.

use std::collections::BTreeSet;

use velme_diagnostics::Code;

#[test]
fn ac_err_01_code_enum_matches_reference_90() {
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/spec/reference/90-errors-glossary.md"
    ))
    .expect("reference/90 is readable");
    let in_spec: BTreeSet<(String, String)> = spec
        .lines()
        .filter(|l| l.starts_with("| VL"))
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            (cells[1].to_owned(), cells[2].to_owned())
        })
        .collect();
    let in_enum: BTreeSet<(String, String)> = Code::ALL
        .iter()
        .map(|c| (c.as_str().to_owned(), c.name().to_owned()))
        .collect();
    assert_eq!(in_enum, in_spec);
    assert_eq!(in_enum.len(), Code::ALL.len(), "no code listed twice");
}
