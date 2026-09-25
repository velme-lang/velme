//! Repository automation behind `cargo xtask` (`delivery/51` §4, `delivery/52` §3).
#![forbid(unsafe_code)]

pub mod ac_audit;
pub mod layering;
pub mod verify;

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The repository root, one level above this crate.
pub fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().context("xtask has no parent directory")?;
    Ok(root.to_path_buf())
}

/// The `cargo` that launched xtask, so steps use the pinned toolchain.
pub fn cargo_program() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}
