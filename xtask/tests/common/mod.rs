//! Shared helpers for xtask integration tests.
// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A fresh, empty directory under Cargo's per-target temp dir, unique per test name.
pub fn fresh_dir(name: &str) -> io::Result<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Writes `contents` to `dir/relative`, creating parent directories.
pub fn write(dir: &Path, relative: &str, contents: &str) -> io::Result<()> {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
