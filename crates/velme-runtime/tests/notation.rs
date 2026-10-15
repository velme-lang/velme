//! Loan builds, artifacts, locks and replay for reply notation (AC-SYNTH-50, AC-SYNTH-54).
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use velme_runtime::{BuildInput, BuildReport, Lock, Mode, Options, Status, Store, build};
use velme_synth::{
    Identity, ProviderError, Recorder, Replay, ReplyFormat, SynthBackend, SynthLimits, SynthOptions, SynthProvider,
    SynthReply, SynthRequest,
};
use velme_test_support::{PanicProvider, program, read, repo};

const SOURCE: &str = include_str!("../../velme-synth/tests/notation/loan/loan.velme");

#[derive(Clone)]
struct LoanBackend {
    format: ReplyFormat,
    version: String,
}

impl LoanBackend {
    fn new(format: ReplyFormat) -> Self {
        let version = match format {
            ReplyFormat::IrJson => "0.1".to_owned(),
            ReplyFormat::Notation => format!("0.1+reply_format=notation@{}", velme_synth::notation::GRAMMAR_VERSION),
            ReplyFormat::Compact => panic!("not a loan fixture format"),
        };
        Self { format, version }
    }
}

#[async_trait]
impl SynthBackend for LoanBackend {
    async fn identify(&self) -> Result<Identity, ProviderError> {
        Ok(Identity {
            provider: "external".to_owned(),
            model: "loan@1".to_owned(),
            input_version: self.version.clone(),
            backend: Some("loan".to_owned()),
        })
    }
    fn open(&self, _: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl SynthProvider for LoanBackend {
    fn id(&self) -> &str {
        "external"
    }
    fn model(&self) -> &str {
        "loan@1"
    }
    fn input_version(&self) -> &str {
        &self.version
    }
    async fn complete(&self, request: &SynthRequest, _: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let dir = repo("crates/velme-synth/tests/notation/loan");
        assert_eq!(
            request.reply_format,
            (self.format != ReplyFormat::IrJson).then_some(self.format)
        );
        let reply_json = if self.format == ReplyFormat::Notation {
            json!({"body":read(&dir.join(format!("{}.notation", request.goal)))}).to_string()
        } else {
            read(&dir.join(format!("{}.json", request.goal)))
        };
        Ok(SynthReply {
            reply_json,
            usage: Default::default(),
            latency: Default::default(),
            meta: Default::default(),
        })
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("notation").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("directory");
    dir
}

fn run(dir: &Path, backend: &dyn SynthBackend, format: ReplyFormat) -> BuildReport {
    let program = program(SOURCE);
    let input = BuildInput {
        program: &program,
        source: SOURCE,
        project: dir,
        file: "loan.velme",
        mode: Mode::Build,
        backend: Some(backend),
        options: SynthOptions {
            reply_format: format,
            max_retries: 0,
            ..SynthOptions::default()
        },
        run: Options::default(),
    };
    let report = build(&input, &mut || {});
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert!(
        report.goals.iter().all(|g| matches!(g.status, Status::Built(_))),
        "{report:?}"
    );
    report
}

fn artifacts(dir: &Path) -> Vec<(String, String)> {
    Lock::read(dir)
        .expect("lock")
        .expect("present")
        .entries()
        .iter()
        .map(|entry| {
            let artifact = Store::new(dir).get(entry.artifact).expect("artifact");
            (
                entry.name.clone(),
                velme_ir::to_canonical_string(&serde_json::to_value(artifact.ir).expect("IR")).expect("canonical IR"),
            )
        })
        .collect()
}

#[test]
fn ac_synth_50_loan_examples_pass_and_artifact_ir_is_identical_between_formats() {
    let notation = scratch("notation");
    let json = scratch("json");
    let notation_report = run(
        &notation,
        &LoanBackend::new(ReplyFormat::Notation),
        ReplyFormat::Notation,
    );
    let json_report = run(&json, &LoanBackend::new(ReplyFormat::IrJson), ReplyFormat::IrJson);
    assert_eq!(
        (notation_report.summary.synthesized, json_report.summary.synthesized),
        (10, 10)
    );
    assert_eq!(artifacts(&notation), artifacts(&json));
    let log = read(&notation.join(".velme/synth-log.jsonl"));
    for line in log.lines() {
        let entry: Value = serde_json::from_str(line).expect("line");
        assert_eq!(entry["reply_format"], "notation");
        assert_eq!(entry["reply_shape"]["keys"], json!(["body"]));
    }
}

#[test]
fn ac_synth_54_recorded_notation_replays_and_a_fresh_lock_makes_zero_calls() {
    let recorded = scratch("recorded");
    let replayed = scratch("replayed");
    let fixtures = scratch("fixtures");
    let options = SynthOptions {
        reply_format: ReplyFormat::Notation,
        max_retries: 0,
        ..SynthOptions::default()
    };
    let backend = Recorder::new(Box::new(LoanBackend::new(ReplyFormat::Notation)), &fixtures).with_options(&options);
    run(&recorded, &backend, ReplyFormat::Notation);
    let identity: Value = serde_json::from_str(&read(&fixtures.join("replay.json"))).expect("identity");
    assert_eq!(identity["reply_format"], "notation");
    let replay = Replay::new(&fixtures);
    run(&replayed, &replay, ReplyFormat::Notation);
    assert_eq!(artifacts(&recorded), artifacts(&replayed));
    let lock = run(&recorded, &PanicProvider, ReplyFormat::Notation);
    assert_eq!((lock.summary.calls, lock.summary.lock_hits), (0, 10));
}
