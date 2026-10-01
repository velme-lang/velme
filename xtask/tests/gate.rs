//! `cargo xtask gate` (`delivery/51` R-QA-07, D-130): what it runs, and how it finds each `ac_rdm_*` test.

use xtask::gate::{self, RUNS};

#[test]
fn r_qa_07_gate_runs_every_perf_test_in_release_one_at_a_time() {
    let steps = gate::perf_steps();
    let args: Vec<String> = steps.iter().map(|s| s.args.join(" ")).collect();
    assert_eq!(
        steps.iter().map(|s| s.name).collect::<Vec<_>>(),
        ["perf", "perf-wasm-ratio"]
    );
    let tail = "--ignored --nocapture --test-threads=1";
    assert_eq!(
        args[0],
        format!(
            "test --release --no-fail-fast -p velme-sema -p velme-ir -p velme-runtime -p velme-cli --test perf -- \
             --skip {} {tail}",
            gate::WASM_RATIO_TEST
        )
    );
    // The WASM ratio runs alone, in a process of its own (D-130).
    assert_eq!(
        args[1],
        format!(
            "test --release --no-fail-fast -p velme-runtime --test perf -- --exact {} {tail}",
            gate::WASM_RATIO_TEST
        )
    );
    assert_eq!(RUNS, 20);
}

#[test]
fn r_qa_07_gate_finds_the_ac_rdm_tests_of_a_binary_by_their_last_segment() {
    let list = "ac_rdm_01_one: test\ntests::ac_rdm_09_two: test\nac_run_01_other: test\nnot_ac_rdm_03: test\n\
                ac_rdm_04_bench: benchmark\n2 tests, 0 benchmarks\n";
    assert_eq!(gate::rdm_tests(list), ["ac_rdm_01_one", "tests::ac_rdm_09_two"]);
}

#[test]
fn r_qa_07_gate_reads_the_test_binaries_from_cargos_messages() {
    let messages = [
        r#"{"reason":"compiler-artifact","profile":{"test":true},"executable":"/t/deps/run-1","manifest_path":"/r/crates/a/Cargo.toml"}"#,
        r#"{"reason":"compiler-artifact","profile":{"test":false},"executable":"/t/velme","manifest_path":"/r/crates/a/Cargo.toml"}"#,
        r#"{"reason":"compiler-artifact","profile":{"test":true},"executable":null,"manifest_path":"/r/crates/b/Cargo.toml"}"#,
        r#"{"reason":"build-finished","success":true}"#,
    ]
    .join("\n");
    let binaries = gate::test_binaries(&messages).expect("parses");
    assert_eq!(binaries.len(), 1);
    assert_eq!(binaries[0].executable, std::path::Path::new("/t/deps/run-1"));
    assert_eq!(binaries[0].dir, std::path::Path::new("/r/crates/a"));
}
