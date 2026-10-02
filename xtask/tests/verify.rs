//! Verify gate tests (AC-QA-01).

use xtask::verify::{self, Step};
use xtask::workspace_root;

#[test]
fn ac_qa_01_verify_runs_every_gate_step() {
    let names: Vec<&str> = verify::steps(false).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "fmt",
            "clippy",
            "clippy-cli",
            "test",
            "doc",
            "deny",
            "layering",
            "features",
            "ac-audit"
        ]
    );
}

/// From M8's exit the audit fails on a criterion with no test (D-139).
#[test]
fn ac_qa_03_verify_runs_the_audit_strict() {
    let steps = verify::steps(false).expect("steps");
    let audit = steps.iter().find(|s| s.name == "ac-audit").expect("an ac-audit step");
    assert_eq!(audit.args, ["ac-audit", "--strict"]);
}

#[test]
fn ac_qa_01_quick_verify_skips_the_slow_steps() {
    let names: Vec<&str> = verify::steps(true).expect("steps").iter().map(|s| s.name).collect();
    assert_eq!(names, ["fmt", "clippy", "clippy-cli", "test"]);
}

#[test]
fn ac_qa_01_verify_reports_a_failing_step_and_still_runs_the_rest() {
    let cargo = xtask::cargo_program();
    let step = |name, arg: &str| Step {
        name,
        program: cargo.clone(),
        args: vec![arg.to_owned()],
        envs: Vec::new(),
        one_test: false,
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
        "VELME_EXTERNAL_URL",
        "VELME_EXTERNAL_TOKEN",
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

/// The release-feature check finds the test endpoint in a `cargo tree` listing and nothing else, and passes on the
/// workspace as it is: `velme-cli`'s normal dependencies never enable it.
#[test]
fn ac_qa_01_the_features_check_flags_only_the_test_endpoint() {
    let tree = "velme-cli v0.1.0\n└── velme-synth feature \"provider-ollama\"\nvelme-synth feature \"test-endpoint\"\n";
    assert_eq!(
        xtask::features::offending_lines(tree),
        ["velme-synth feature \"test-endpoint\""]
    );
    assert!(xtask::features::offending_lines("velme-synth feature \"provider-ollama\"\n").is_empty());
    assert_eq!(
        xtask::features::check(&workspace_root().expect("root")).expect("cargo tree"),
        Vec::<String>::new()
    );
}

/// Each step gets a fresh empty home, which is gone afterwards: the second step finds nothing the first left in it
/// (AC-QA-02).
#[cfg(unix)]
#[test]
fn ac_qa_02_each_step_gets_a_fresh_empty_home_removed_afterwards() {
    let record = std::env::temp_dir().join(format!("velme-gate-home-record-{}", std::process::id()));
    let _ = std::fs::remove_file(&record);
    let script = format!(
        "test -z \"$(ls -A \"$HOME\")\" && touch \"$HOME/left-behind\" && echo \"$HOME\" >> {}",
        record.display()
    );
    let step = |name| Step {
        name,
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.clone()],
        envs: Vec::new(),
        one_test: false,
    };
    let failed = verify::run_steps(&workspace_root().expect("root"), &[step("one"), step("two")]);
    assert!(failed.is_empty(), "{failed:?}");
    let homes = std::fs::read_to_string(&record).expect("recorded");
    let homes: Vec<&str> = homes.lines().collect();
    assert_eq!(homes.len(), 2);
    assert_ne!(homes[0], homes[1]);
    assert!(homes.iter().all(|h| !std::path::Path::new(h).exists()), "{homes:?}");
    let _ = std::fs::remove_file(&record);
}
