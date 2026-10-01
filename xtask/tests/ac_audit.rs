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

/// Only a `#[test]` function counts (D-139): an id in a doc comment, a block comment, a string or a raw string, or a
/// helper `fn ac_…` with no `#[test]`, covers nothing, while further attributes and qualifiers (`pub(crate) async`) don't
/// hide the function, and an escaped quote (`'\''`) ends its character literal.
/// The fake functions are in string literals here, so the audit of this repository doesn't see them either.
#[test]
fn ac_qa_03_audit_counts_test_functions_only() {
    let dir = common::fresh_dir("ac_audit_test_functions_only").expect("fresh dir");
    common::write(&dir, "docs/spec/x.md", SPEC_TABLE).expect("write fixture");
    let not_tests = "/// #[test]\n/// fn ac_xyz_09_in_a_doc_comment() {}\n\
                     /* #[test]\nfn ac_xyz_09_in_a_block_comment() {} */\n\
                     fn ac_xyz_09_a_helper() {}\n\
                     const S: &str = \"#[test]\\nfn ac_xyz_09_in_a_string() {}\";\n\
                     const R: &str = r#\"\n#[test]\nfn ac_xyz_09_in_a_raw_string() {}\n\"#;\n\
                     const Q: char = '\"';\n\
                     const E: [char; 2] = ['\\'','\"'];\n";
    let tests = "#[test]\n#[ignore = \"slow\"]\n#[cfg(unix)]\nfn ac_xyz_01_behind_more_attributes() {}\n\
                 #[test]\npub(crate) async fn ac_xyz_02_qualified() {}\n\
                 #[test]\npub unsafe fn ac_xyz_03_qualified() {}\n";
    common::write(&dir, "crates/a/src/lib.rs", &format!("{not_tests}{tests}")).expect("write fixture");
    let report = ac_audit::audit(&dir).expect("audit");
    assert_eq!(
        report.covered.into_iter().collect::<Vec<_>>(),
        ["AC-XYZ-01", "AC-XYZ-02"]
    );
    assert_eq!(report.unknown.into_iter().collect::<Vec<_>>(), ["AC-XYZ-03"]);
}
