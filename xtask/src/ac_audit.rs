//! Coverage audit (`delivery/51` §5): every spec `AC-*` has a `#[test]` function named for it.
//!
//! Uncovered criteria are listed, and fail only under `--strict` (AC-QA-03, D-139); tests citing an unknown criterion,
//! and ignored tests outside the perf files `cargo xtask gate` runs, always fail.

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
    /// Ignored `ac_*` tests outside a [`crate::gate::PERF_CRATES`] `tests/perf.rs`, which no gate runs: (file relative
    /// to the root, test name).
    pub ignored: BTreeSet<(PathBuf, String)>,
}

impl Report {
    /// Defined criteria with no test yet.
    pub fn uncovered(&self) -> BTreeSet<String> {
        self.defined.difference(&self.covered).cloned().collect()
    }

    /// Whether the audit passes; `strict` also requires every criterion to be covered.
    pub fn passes(&self, strict: bool) -> bool {
        self.unknown.is_empty() && self.ignored.is_empty() && (!strict || self.covered.len() == self.defined.len())
    }
}

/// Audits the repository at `root`.
pub fn audit(root: &Path) -> Result<Report> {
    let mut defined = BTreeSet::new();
    for file in files_with_extension(&root.join(SPEC_ROOT), "md")? {
        let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        defined.extend(text.lines().filter_map(criterion_in_table_row));
    }
    let (mut cited, mut ignored) = (BTreeSet::new(), BTreeSet::new());
    for dir in TEST_ROOTS {
        for file in files_with_extension(&root.join(dir), "rs")? {
            let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let relative = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
            let (ids, misplaced) = test_criteria(&text, is_perf_file(&relative));
            cited.extend(ids);
            ignored.extend(misplaced.into_iter().map(|name| (relative.clone(), name)));
        }
    }
    let covered = defined.intersection(&cited).cloned().collect();
    let unknown = cited.difference(&defined).cloned().collect();
    Ok(Report {
        defined,
        covered,
        unknown,
        ignored,
    })
}

/// Whether `file` is `crates/<c>/tests/perf.rs` for a crate whose perf tests `cargo xtask gate` runs, ignored ones
/// included (D-139).
fn is_perf_file(file: &Path) -> bool {
    let parts: Vec<_> = file.iter().filter_map(|p| p.to_str()).collect();
    matches!(parts.as_slice(), ["crates", c, "tests", "perf.rs"] if crate::gate::PERF_CRATES.contains(c))
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

/// Criteria named by `#[test] fn ac_<area>_<nn>…` functions (R-QA-01, CC-TEST-01). Only a function under a `#[test]`
/// attribute counts, and comments and string literals are blanked first, so neither a helper `fn ac_…` nor an id quoted
/// in a doc comment or a string covers a criterion (D-139). An `#[ignore]`d one counts only where `ignored_runs` (a perf
/// file the gate runs); elsewhere its name is returned second, as an error.
fn test_criteria(source: &str, ignored_runs: bool) -> (BTreeSet<String>, Vec<String>) {
    let code = code_only(source);
    let (mut ids, mut misplaced) = (BTreeSet::new(), Vec::new());
    for (at, attribute) in code.match_indices("#[test]") {
        let Some((name, ignored_after)) = code.get(at + attribute.len()..).and_then(function_after_attributes) else {
            continue;
        };
        // The attributes before `#[test]` belong to the same function: the run back to the previous item's end.
        let before = code.get(..at).unwrap_or_default();
        let run = before
            .get(before.rfind([';', '{', '}']).map_or(0, |i| i + 1)..)
            .unwrap_or_default();
        let ignored = ignored_after || run.contains("#[ignore");
        let mut parts = name.split('_');
        if let (Some("ac"), Some(area), Some(num)) = (parts.next(), parts.next(), parts.next())
            && !area.is_empty()
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            if ignored && !ignored_runs {
                misplaced.push(name.clone());
            } else {
                ids.insert(format!("AC-{}-{num}", area.to_ascii_uppercase()));
            }
        }
    }
    (ids, misplaced)
}

/// The name of the `fn` that `code` starts with once any further attributes (`#[ignore]`, `#[cfg(…)]`) and qualifiers
/// are skipped, and whether one of them was `#[ignore]`.
fn function_after_attributes(mut code: &str) -> Option<(String, bool)> {
    let mut ignored = false;
    loop {
        code = code.trim_start();
        let Some(attribute) = code.strip_prefix("#[") else {
            break;
        };
        let mut depth = 1usize;
        let end = attribute.char_indices().find_map(|(i, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        })?;
        let inner = attribute.get(..end)?.trim_start();
        ignored |= inner
            .strip_prefix("ignore")
            .is_some_and(|r| r.trim_start().is_empty() || r.trim_start().starts_with('='));
        code = attribute.get(end + 1..)?;
    }
    // Qualifiers before `fn`: `pub`, `pub(crate)`, `async`, `const`, `unsafe`, `extern` (its ABI string is blanked).
    loop {
        code = code.trim_start();
        if let Some(rest) = code.strip_prefix("pub") {
            let rest = rest.trim_start();
            code = match rest.strip_prefix('(') {
                Some(scope) => scope.get(scope.find(')')? + 1..)?,
                None => rest,
            };
            continue;
        }
        let Some(rest) = ["async", "const", "unsafe", "extern"]
            .iter()
            .find_map(|q| code.strip_prefix(q).filter(|r| r.starts_with(char::is_whitespace)))
        else {
            break;
        };
        code = rest;
    }
    let rest = code.strip_prefix("fn")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    Some((name, ignored))
}

/// `source` with every comment and every string, raw string and character literal replaced by spaces.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let ident = |i: usize| chars.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        let start = i;
        match c {
            '/' if next == Some('/') => {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                let mut depth = 0usize;
                loop {
                    match (chars.get(i), chars.get(i + 1)) {
                        (Some('/'), Some('*')) => {
                            depth += 1;
                            i += 2;
                        }
                        (Some('*'), Some('/')) => {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        }
                        (Some(_), _) => i += 1,
                        (None, _) => break,
                    }
                }
            }
            '"' => {
                i += 1;
                while let Some(&c) = chars.get(i) {
                    i += if c == '\\' { 2 } else { 1 };
                    if c == '"' {
                        break;
                    }
                }
            }
            // `r"…"`, `r#"…"#` and `br"…"`, but not the `r` ending a name.
            'r' if !ident(i.wrapping_sub(1))
                || (chars.get(i.wrapping_sub(1)) == Some(&'b') && !ident(i.wrapping_sub(2))) =>
            {
                let hashes = chars.iter().skip(i + 1).take_while(|c| **c == '#').count();
                if chars.get(i + 1 + hashes) != Some(&'"') {
                    out.push(c);
                    i += 1;
                    continue;
                }
                i += hashes + 2;
                while let Some(&c) = chars.get(i) {
                    i += 1;
                    if c == '"' && chars.get(i..i + hashes).is_some_and(|h| h.iter().all(|c| *c == '#')) {
                        i += hashes;
                        break;
                    }
                }
            }
            // A character literal (`'x'`, `'\n'`, `'"'`); a lifetime (`'a`) is code.
            '\'' if next == Some('\\') => {
                // Past the quote, the backslash and the escaped character, which may itself be a quote (`'\''`).
                i += 3;
                while chars.get(i).is_some_and(|c| *c != '\'') {
                    i += 1;
                }
                i += 1;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') => i += 3,
            _ => {
                out.push(c);
                i += 1;
                continue;
            }
        }
        out.extend(std::iter::repeat_n(' ', i.min(chars.len()) - start));
    }
    out
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
