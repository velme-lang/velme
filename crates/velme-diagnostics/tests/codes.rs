//! The code enum matches `reference/90` §2 exactly, and each code it lists has a human and a JSON snapshot.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use velme_diagnostics::Code;

/// The workspace root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each `(code, name)` row of `reference/90` §2.
fn listed() -> BTreeSet<(String, String)> {
    let spec = std::fs::read_to_string(root().join("docs/spec/reference/90-errors-glossary.md"))
        .expect("reference/90 is readable");
    spec.lines()
        .filter(|l| l.starts_with("| VL"))
        .map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            (cells[1].to_owned(), cells[2].to_owned())
        })
        .collect()
}

#[test]
fn ac_err_01_code_enum_matches_reference_90() {
    let in_spec = listed();
    let in_enum: BTreeSet<(String, String)> = Code::ALL
        .iter()
        .map(|c| (c.as_str().to_owned(), c.name().to_owned()))
        .collect();
    assert_eq!(in_enum, in_spec);
    assert_eq!(in_enum.len(), Code::ALL.len(), "no code listed twice");
}

/// Every accepted `.snap` file under `dir`, in any `snapshots` directory below it.
fn snapshots(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("directory").filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            snapshots(&path, found);
        } else if path.extension().is_some_and(|e| e == "snap")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "snapshots")
        {
            found.push(path);
        }
    }
}

/// Each `VLnnnn` in `text`: `VL` and four digits, with no letter or digit on either side.
fn codes_in(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let word = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_alphanumeric);
    text.match_indices("VL")
        .filter(|&(i, _)| {
            (i == 0 || !word(i - 1))
                && bytes
                    .get(i + 2..i + 6)
                    .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
                && !word(i + 6)
        })
        .filter_map(|(i, _)| text.get(i..i + 6))
        .collect()
}

/// AC-QA-04 (D-140, R-QA-05): the codes `reference/90` lists are exactly the codes the snapshots show, each in a human
/// rendering (a headline `Error: …  [VLnnnn]` or `Warning: …  [VLnnnn]`) and in a JSON one (`"code": "VLnnnn"`); and
/// no snapshot names a code that isn't listed.
#[test]
fn ac_qa_04_every_listed_code_has_a_human_and_a_json_snapshot() {
    let listed: BTreeSet<String> = listed().into_iter().map(|(code, _)| code).collect();
    let mut files = Vec::new();
    snapshots(&root().join("crates"), &mut files);
    files.sort();
    let mut human = BTreeSet::new();
    let mut json = BTreeSet::new();
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a snapshot is text");
        for line in text.lines() {
            let line = line.trim();
            if (line.starts_with("Error: ") || line.starts_with("Warning: "))
                && let Some((_, code)) = line.strip_suffix(']').and_then(|l| l.rsplit_once("  ["))
            {
                human.insert(code.to_owned());
            }
            if let Some(code) = line.strip_prefix("\"code\": \"") {
                json.extend(codes_in(code).first().map(|c| (*c).to_owned()));
            }
        }
        for code in codes_in(&text) {
            let crates = root().join("crates");
            let shown = file.strip_prefix(&crates).unwrap_or(file).display().to_string();
            named.entry(code.to_owned()).or_insert(shown);
        }
    }
    assert!(!files.is_empty(), "no snapshots found");
    let crates = root().join("crates");
    let orphans: Vec<&Path> = files
        .iter()
        .filter(|f| !live(f))
        .map(|f| f.strip_prefix(&crates).unwrap_or(f))
        .collect();
    assert!(
        orphans.is_empty(),
        "snapshots no test writes any more (delete them): {orphans:?}"
    );
    let no_human: Vec<&String> = listed.difference(&human).collect();
    let no_json: Vec<&String> = listed.difference(&json).collect();
    assert!(no_human.is_empty(), "codes with no human snapshot: {no_human:?}");
    assert!(no_json.is_empty(), "codes with no JSON snapshot: {no_json:?}");
    let unlisted: Vec<(&String, &String)> = named.iter().filter(|(c, _)| !listed.contains(*c)).collect();
    assert!(
        unlisted.is_empty(),
        "snapshots naming codes reference/90 doesn't list: {unlisted:?}"
    );
}

/// Whether a test still writes the snapshot `file`. Insta names it `<module>__<name>[-N][@suffix].snap`, the module's
/// source sitting beside the `snapshots` directory as `<module>.rs` (its last path segment). `<name>` is a test's own
/// name (less a `test_` prefix) or one the test passes: a literal `"<name>"`, or a `format!` whose holes the name
/// fills with text the source also quotes (`format!("{code}_human")` with `"VL0403"`). A `@suffix` from `glob!` names
/// a file under `tests/golden`, which must still be there.
fn live(file: &Path) -> bool {
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("a UTF-8 snapshot name");
    let (stem, suffix) = stem.split_once('@').map_or((stem, None), |(s, x)| (s, Some(x)));
    let Some((module, name)) = stem.rsplit_once("__") else {
        return false;
    };
    let name = uncounted(name);
    let module = module.rsplit("__").next().unwrap_or(module);
    let dir = file
        .parent()
        .and_then(Path::parent)
        .expect("a snapshots directory has a parent");
    let Ok(source) = std::fs::read_to_string(dir.join(format!("{module}.rs"))) else {
        return false;
    };
    let named = [format!("fn {name}("), format!("fn test_{name}("), format!("\"{name}\"")]
        .iter()
        .any(|n| source.contains(n.as_str()))
        || formats(&source).iter().any(|f| fills(f, name, &source));
    named && suffix.is_none_or(|s| golden(&root().join("tests/golden"), uncounted(s)))
}

/// `text` less insta's `-N` counter for a test's second and later unnamed snapshots.
fn uncounted(text: &str) -> &str {
    text.rsplit_once('-')
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(text, |(t, _)| t)
}

/// The literal of each `format!("…")` in `source`.
fn formats(source: &str) -> Vec<&str> {
    source
        .match_indices("format!(\"")
        .filter_map(|(i, m)| {
            let rest = source.get(i + m.len()..)?;
            rest.find('"').and_then(|end| rest.get(..end))
        })
        .collect()
}

/// Whether `name` is `pattern` with each `{…}` hole filled by text `source` quotes, ignoring case.
fn fills(pattern: &str, name: &str, source: &str) -> bool {
    let quoted = source.to_lowercase();
    let mut parts = Vec::new();
    let mut rest = pattern;
    while let Some((before, after)) = rest.split_once('{') {
        parts.push(before);
        rest = after.split_once('}').map_or("", |(_, a)| a);
    }
    parts.push(rest);
    if parts.len() < 2 {
        return false;
    }
    let mut at = name;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = at.strip_prefix(part) else { return false };
            at = r;
            continue;
        }
        // The hole before `part` takes the shortest text that leaves `part` next, or all that is left if it ends.
        let end = if i + 1 == parts.len() && part.is_empty() {
            at.len()
        } else if i + 1 == parts.len() {
            match at.strip_suffix(part) {
                Some(r) => r.len(),
                None => return false,
            }
        } else {
            match at.find(part) {
                Some(e) => e,
                None => return false,
            }
        };
        let hole = &at[..end];
        if hole.is_empty() || !quoted.contains(&format!("\"{}\"", hole.to_lowercase())) {
            return false;
        }
        at = &at[end + part.len()..];
    }
    at.is_empty()
}

/// Whether some file under `dir` has the path `rel` relative to a directory below it.
fn golden(dir: &Path, rel: &str) -> bool {
    std::fs::read_dir(dir)
        .expect("directory")
        .filter_map(Result::ok)
        .any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                golden(&path, rel)
            } else {
                path.ends_with(rel)
            }
        })
}
