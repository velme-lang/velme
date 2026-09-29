//! Velme test support (`delivery/51`): helpers shared by the test suites of several crates. A dev-dependency only.
#![forbid(unsafe_code)]
// A helper here fails the test that called it, so it panics on bad input like a test body does (CC-ERR-01 covers
// non-test code only).
#![allow(clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use velme_ir::{
    CallNode, Fingerprint, Goal, Origin, Request, Synthesis, ValidIr, calls, contract_key, from_json_str, signature,
    synthesis_key, validate,
};
use velme_runtime::{ArtifactFormat, Child, Entry, Lock, Manifest, Store, Verification};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};

/// `path`, relative to the repository root.
pub fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

/// The text of the file at `path`.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The checked program of `text`, which must have no errors.
pub fn program(text: &str) -> Program {
    let (program, diags) = analyze(&SourceFile::new("test.velme", text));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

/// The goal called `name`.
pub fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(
        program
            .goals
            .iter()
            .position(|g| g.name == name)
            .unwrap_or_else(|| panic!("no goal {name}")),
    )
}

/// Hand-written IR for a goal of `program`, validated as a whole goal: its `calls` must equal the compiler's
/// (`compiler/21` R-IR-16).
pub fn valid_ir(program: &Program, ir: &str) -> ValidIr {
    let goal: Goal = from_json_str(ir).unwrap_or_else(|e| panic!("IR doesn't parse: {e}"));
    let id = goal_id(program, &goal.goal);
    let calls = calls(program, id).expect("a checked goal has a call section");
    let request = Request {
        program,
        goal: id,
        calls: &calls,
        origin: Origin::Complete,
    };
    validate(ir, &request).unwrap_or_else(|d| panic!("IR doesn't validate: {d:#?}"))
}

/// The backend a fixture's manifest names: hand-written IR comes from the `external` backend (`runtime/32` R-ART-21).
pub const FIXTURE_BACKEND: &str = "velme-test-support";

/// The fixture backend's `backend_version` and `request_version`.
pub const FIXTURE_VERSION: &str = "hand-written";

/// The `external` provider id (`runtime/32` R-ART-21).
const EXTERNAL: &str = "external";

/// Installs hand-written IR for a goal of `program`, declared in the project file `file`, into the store and lock of
/// the project `project` (D-16), and returns the artifact's address. The IR is validated but not verified, so a test
/// can install IR that fails its goal's examples or checks on purpose.
pub fn install(project: &Path, file: &str, program: &Program, ir: &str) -> Fingerprint {
    let ir = valid_ir(program, ir);
    let manifest = fixture_manifest(program, &ir);
    install_artifact(project, file, program, &manifest, &ir)
}

/// The manifest [`install`] writes for `ir`: an `external` artifact of [`FIXTURE_BACKEND`] that ran no verification.
pub fn fixture_manifest(program: &Program, ir: &ValidIr) -> Manifest {
    let goal = ir.goal();
    let id = goal_id(program, &goal.goal);
    let contract = contract_key(program, id).expect("contract key");
    let compiler_version = env!("CARGO_PKG_VERSION");
    let synthesis = Synthesis {
        input_version: FIXTURE_VERSION,
        compiler_version,
        provider: EXTERNAL,
        model: FIXTURE_VERSION,
    };
    Manifest {
        format: ArtifactFormat,
        goal: goal.goal.clone(),
        kind: program.goals.get(id.0).expect("a goal of the program").kind.into(),
        signature: signature(program, id).expect("signature"),
        contract_key: contract,
        synthesis_key: synthesis_key(contract, &synthesis).expect("synthesis key"),
        language_version: program.language_version.clone(),
        compiler_version: compiler_version.to_owned(),
        ir_version: goal.ir_version.clone(),
        builtins_version: goal.builtins_version.clone(),
        prompt_version: Some(FIXTURE_VERSION.to_owned()),
        provider: EXTERNAL.to_owned(),
        backend: Some(FIXTURE_BACKEND.to_owned()),
        model_version: Some(FIXTURE_VERSION.to_owned()),
        children: goal
            .calls
            .iter()
            .map(|CallNode::Call(call)| Child {
                binding: call.binding.clone(),
                goal: call.goal.clone(),
                signature: call.goal_signature.parse().expect("a validated signature"),
            })
            .collect(),
        verification: Verification {
            examples: 0,
            generated_inputs: 0,
            input_set: Fingerprint::of(&[(); 0]).expect("an empty input set"),
            max_fuel_observed: 0,
        },
    }
}

/// Stores `manifest` with `ir` and pins it in the lock of `project` under the keys computed from `program`, whatever
/// the manifest claims, so a test can install a manifest that disagrees with its lock entry (AC-ART-12).
pub fn install_artifact(
    project: &Path,
    file: &str,
    program: &Program,
    manifest: &Manifest,
    ir: &ValidIr,
) -> Fingerprint {
    let id = goal_id(program, &ir.goal().goal);
    let artifact = Store::new(project).put(manifest, ir).expect("artifact stored");
    let mut lock = Lock::read(project)
        .expect("lock readable")
        .unwrap_or_else(|| Lock::new(program.language_version.clone()));
    lock.language = program.language_version.clone();
    lock.insert(Entry {
        file: file.to_owned(),
        name: ir.goal().goal.clone(),
        signature: signature(program, id).expect("signature"),
        contract_key: contract_key(program, id).expect("contract key"),
        artifact,
    });
    lock.write(project).expect("lock written");
    artifact
}
