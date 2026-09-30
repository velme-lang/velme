//! The local synth log (`compiler/22` R-SYNTH-23, D-109): one JSON line per provider request in
//! `.velme/synth-log.jsonl`. It holds no plan text, prompt, reply or value, and is written only through a regular file.

use std::path::Path;

use velme_ir::Fingerprint;
use velme_synth::CallRecord;
use velme_synth::fsio::{append_file, create_file_if_absent};

use crate::store::VELME_DIR;

/// The log's file name inside [`VELME_DIR`].
pub const SYNTH_LOG_FILE: &str = "synth-log.jsonl";

/// What `.velme/.gitignore` holds: the log, and nothing else (D-109).
const GITIGNORE: &str = "synth-log.jsonl\n";

/// The `format` member of every line.
const FORMAT: &str = "velme-synth-log/1";

/// What the lines of one goal have in common.
pub(crate) struct Goal<'a> {
    pub file: &'a str,
    pub goal: &'a str,
    pub key: Fingerprint,
    pub provider: &'a str,
    pub model: &'a str,
}

/// Appends a line for each of `records`. An error is for the caller to show as a `-v` notice, never to fail the build.
pub(crate) fn append(project: &Path, goal: &Goal<'_>, records: &[CallRecord]) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    let dir = project.join(VELME_DIR);
    let written = create_file_if_absent(&dir, ".gitignore", GITIGNORE.as_bytes()).and_then(|()| {
        records
            .iter()
            .try_for_each(|record| append_file(&dir, SYNTH_LOG_FILE, line(goal, record).as_bytes()))
    });
    written.map_err(|e| format!("the synth log `{VELME_DIR}/{SYNTH_LOG_FILE}` wasn't written: {e}"))
}

/// One line, keys in the order of R-SYNTH-23, no spaces, ending in a newline.
fn line(goal: &Goal<'_>, record: &CallRecord) -> String {
    let text = |s: &str| serde_json::Value::from(s).to_string();
    let count = |n: Option<u64>| n.map_or_else(|| "null".to_owned(), |n| n.to_string());
    let (tokens_in, tokens_out) = record.tokens;
    format!(
        "{{\"format\":\"{FORMAT}\",\"time\":\"{}\",\"file\":{},\"goal\":{},\"key\":\"{}\",\"provider\":{},\"model\":{},\"attempt\":{},\"outcome\":{},\"tokens_in\":{},\"tokens_out\":{},\"latency_ms\":{}}}\n",
        rfc3339(record.time_millis),
        text(goal.file),
        text(goal.goal),
        goal.key,
        text(goal.provider),
        text(goal.model),
        record.attempt,
        text(&record.outcome),
        count(tokens_in),
        count(tokens_out),
        record.latency.as_millis(),
    )
}

/// `millis` since the Unix epoch as `2026-09-30T12:00:00.000Z`.
fn rfc3339(millis: i64) -> String {
    let (secs, ms) = (millis.div_euclid(1000), millis.rem_euclid(1000));
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_780_142_400_123), "2026-05-30T12:00:00.123Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
