//! Coverage audit stub (`delivery/51` §5): every spec `AC-*` should have a test named for it.
//!
//! The plan-phase exemption for criteria owned by later phases is not implemented yet, so uncovered criteria
//! are reported and fail only under `--strict` (AC-QA-03); tests citing an unknown criterion always fail.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Directories (relative to the repository root) scanned for test function names.
const TEST_ROOTS: &[&str] = &["crates", "tests", "xtask/tests"];
const SPEC_ROOT: &str = "docs/spec";

/// Result of one audit run; ids are upper-case (`AC-REL-02`).
pub struct Report {
    /// Every criterion defined in a spec acceptance table.
    pub defined: BTreeSet<String>,
    /// Defined criteria with at least one test.
    pub covered: BTreeSet<String>,
    /// Criteria cited by a test but defined nowhere in the spec.
    pub unknown: BTreeSet<String>,
}

impl Report {
    /// Defined criteria with no test yet.
    pub fn uncovered(&self) -> BTreeSet<String> {
        self.defined.difference(&self.covered).cloned().collect()
    }

    /// Whether the audit passes; `strict` also requires every criterion to be covered.
    pub fn passes(&self, strict: bool) -> bool {
        self.unknown.is_empty() && (!strict || self.covered.len() == self.defined.len())
    }
}

/// Audits the repository at `root`.
pub fn audit(root: &Path) -> Result<Report> {
    let mut defined = BTreeSet::new();
    for file in files_with_extension(&root.join(SPEC_ROOT), "md")? {
        let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        defined.extend(text.lines().filter_map(criterion_in_table_row));
    }
    let mut cited = BTreeSet::new();
    for dir in TEST_ROOTS {
        for file in files_with_extension(&root.join(dir), "rs")? {
            let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            cited.extend(test_criteria(&text));
        }
    }
    let covered = defined.intersection(&cited).cloned().collect();
    let unknown = cited.difference(&defined).cloned().collect();
    Ok(Report {
        defined,
        covered,
        unknown,
    })
}

/// `| AC-REL-02 | … |` → `AC-REL-02`.
fn criterion_in_table_row(line: &str) -> Option<String> {
    let id = line.strip_prefix("| ")?.split([' ', '|']).next()?;
    let mut parts = id.split('-');
    let well_formed = parts.next() == Some("AC")
        && parts
            .next()
            .is_some_and(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_uppercase()))
        && parts
            .next()
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && parts.next().is_none();
    well_formed.then(|| id.to_owned())
}

/// Criteria named by `fn ac_<area>_<nn>…` test functions (R-QA-01, CC-TEST-01).
fn test_criteria(source: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for (at, _) in source.match_indices("fn ac_") {
        let name: String = source
            .get(at + 3..)
            .unwrap_or_default()
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        let mut parts = name.split('_');
        if let (Some("ac"), Some(area), Some(num)) = (parts.next(), parts.next(), parts.next())
            && !area.is_empty()
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            ids.insert(format!("AC-{}-{num}", area.to_ascii_uppercase()));
        }
    }
    ids
}

/// Files with `extension` under `dir`, sorted; a missing directory is empty. Skips `target/`.
fn files_with_extension(dir: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(files);
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("stat {}", path.display()))?
            .is_dir()
        {
            if path.file_name().is_some_and(|n| n != "target") {
                files.extend(files_with_extension(&path, extension)?);
            }
        } else if path.extension().is_some_and(|e| e == extension) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
