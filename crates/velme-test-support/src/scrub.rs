//! The provider settings no test run sees, shared by `cargo xtask` (which compiles this file into itself, having no
//! dependency on this crate) and the release-mode targets that start the `velme` binary (D-130).

use std::path::Path;
use std::process::Command;

/// The provider settings never let into a run, so it goes as it does with no key, no provider and no live tests
/// (`delivery/51` AC-QA-02, D-13); every variable ending in `_API_KEY` goes too.
pub const PROVIDER_ENV: [&str; 7] = [
    "VELME_MODEL",
    "VELME_EXTERNAL_URL",
    "VELME_OLLAMA_URL",
    "VELME_EXTERNAL_TOKEN",
    "VELME_SYNTH_RECORD",
    "VELME_SYNTH_SCRIPT",
    "VELME_LIVE_LLM",
];

/// Removes from `command`'s environment everything a provider could be reached with, and points the user-level config
/// (`$XDG_CONFIG_HOME`, `%APPDATA%`, and `HOME` for the platform equivalent) at `home`, an empty directory, so a run never
/// reads the developer's own settings (AC-QA-02). The toolchain keeps the homes it had.
pub fn scrub_provider_env(command: &mut Command, home: &Path) {
    let old_home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for (var, dir) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
        if std::env::var_os(var).is_none()
            && let Some(old) = &old_home
        {
            command.env(var, old.join(dir));
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home);
    for name in PROVIDER_ENV {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().to_ascii_uppercase().ends_with("_API_KEY") {
            command.env_remove(name);
        }
    }
}
