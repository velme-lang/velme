//! Coverage audit tests (AC-QA-03).

mod common;

use xtask::ac_audit;

/// A test function for `name`; built at run time so this file's own source never cites the fake criteria.
fn test_fn(name: &str) -> String {
    format!("#[test]\nfn ac_{name}() {{}}\n")
}

const SPEC_TABLE: &str = "| ID | Criterion |\n|---|---|\n| AC-XYZ-01 | first |\n| AC-XYZ-02 | second |\n";

#[test]
fn ac_qa_03_audit_fails_strict_when_a_criterion_has_no_test() {
    let dir = common::fresh_dir("ac_audit_uncovered").expect("fresh dir");
    common::write(&dir, "docs/spec/x.md", SPEC_TABLE).expect("write fixture");
    common::write(&dir, "crates/a/src/lib.rs", &test_fn("xyz_01_first_works")).expect("write fixture");
    let report = ac_audit::audit(&dir).expect("audit");
    assert_eq!(report.uncovered().into_iter().collect::<Vec<_>>(), ["AC-XYZ-02"]);
    assert!(report.passes(false));
    assert!(!report.passes(true));
}

#[test]
fn ac_qa_03_audit_fails_on_a_test_citing_an_unknown_criterion() {
    let dir = common::fresh_dir("ac_audit_unknown").expect("fresh dir");
    common::write(&dir, "docs/spec/x.md", SPEC_TABLE).expect("write fixture");
    common::write(&dir, "tests/t.rs", &test_fn("xyz_09_ghost")).expect("write fixture");
    let report = ac_audit::audit(&dir).expect("audit");
    assert_eq!(report.unknown.into_iter().collect::<Vec<_>>(), ["AC-XYZ-09"]);
}

#[test]
fn ac_qa_03_audit_passes_strict_when_every_criterion_is_covered() {
    let dir = common::fresh_dir("ac_audit_covered").expect("fresh dir");
    common::write(&dir, "docs/spec/x.md", SPEC_TABLE).expect("write fixture");
    common::write(
        &dir,
        "crates/a/src/lib.rs",
        &(test_fn("xyz_01_a") + &test_fn("xyz_02_b")),
    )
    .expect("write fixture");
    assert!(ac_audit::audit(&dir).expect("audit").passes(true));
}
