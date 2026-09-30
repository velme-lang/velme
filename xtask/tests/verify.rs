//! Verify gate tests (AC-QA-01).

use xtask::verify::{self, Step};
use xtask::workspace_root;

#[test]
fn ac_qa_01_verify_runs_every_gate_step() {
    let names: Vec<&str> = verify::steps(false).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(names, ["fmt", "clippy", "test", "doc", "deny", "layering", "ac-audit"]);
}

#[test]
fn ac_qa_01_quick_verify_skips_the_slow_steps() {
    let names: Vec<&str> = verify::steps(true).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(names, ["fmt", "clippy", "test"]);
}

#[test]
fn ac_qa_01_verify_reports_a_failing_step_and_still_runs_the_rest() {
    let cargo = xtask::cargo_program();
    let step = |name, arg: &str| Step {
        name,
        program: cargo.clone(),
        args: vec![arg.to_owned()],
        envs: Vec::new(),
    };
    let steps = [step("bad", "--no-such-flag"), step("good", "--version")];
    let failed = verify::run_steps(&workspace_root().expect("root"), &steps);
    assert_eq!(failed, ["bad"]);
}

/// The gate runs its steps with no key and no provider setting in the environment, and no live tests (AC-QA-02, D-13); the
/// tests themselves reach only servers on this machine.
#[test]
fn ac_qa_02_the_gate_removes_keys_and_provider_settings_from_every_step() {
    let mut command = std::process::Command::new("true");
    let home = std::env::temp_dir().join("velme-gate-test-home");
    verify::scrub_provider_env(&mut command, &home);
    let removed: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in [
        "VELME_MODEL",
        "VELME_EXTERNAL_COMMAND",
        "VELME_SYNTH_RECORD",
        "VELME_SYNTH_SCRIPT",
        "VELME_LIVE_LLM",
    ] {
        assert!(removed.iter().any(|r| r == name), "{name} is left in the environment");
    }
    // The user-level config is an empty directory, not the developer's.
    let set: Vec<(String, Option<String>)> = command
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    for var in ["HOME", "XDG_CONFIG_HOME", "APPDATA"] {
        assert!(
            set.iter().any(|(k, v)| k == var && v.as_deref() == home.to_str()),
            "{var}"
        );
    }
    // Whichever `*_API_KEY` variable the user has set goes too.
    let has_key =
        std::env::vars_os().any(|(name, _)| name.to_string_lossy().to_ascii_uppercase().ends_with("_API_KEY"));
    let removed_keys = removed.iter().any(|r| r.to_ascii_uppercase().ends_with("_API_KEY"));
    assert_eq!(has_key, removed_keys);
}
