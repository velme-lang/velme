//! The two renderers (`compiler/20` R-CMP-15): human text through ariadne, and the JSON shape of `tooling/40` R-CLI-08
//! (D-68). Both take one file's diagnostics together with its path and text; neither prints.

use std::fmt::Write as _;
use std::ops::Range;

use ariadne::{Config, IndexType, Label as AriadneLabel, Report, ReportKind, Source};
use serde::Serialize;

use crate::{Code, Diagnostic, Label, Severity, Span};

/// Human output shows at most this many diagnostics, then "…and N more" (`language/10` R-SYN-17).
pub const MAX_SHOWN: usize = 20;

/// The primary label's text; ariadne draws the caret only under a label that has some (R-CLI-09 style guide).
const PRIMARY_LABEL: &str = "here";

/// Renders diagnostics for a person. `text` is the file's text, or `None` when it couldn't be read; `color` adds ANSI
/// colour. Each diagnostic's first line ends with `[VLnnnn]` (AC-ERR-03). Past [`MAX_SHOWN`] the rest are counted.
///
/// Every string is escaped before display (`tooling/40` R-CLI-17): control and bidi characters in messages become
/// visible `\u{..}` escapes, and in the quoted source become `�`, so no such character reaches the terminal.
pub fn render_human(diagnostics: &[Diagnostic], path: &str, text: Option<&str>, color: bool) -> String {
    let path = escape(path);
    let mut out = String::new();
    let quoted = text.map(Quoted::new);
    for diag in diagnostics.iter().take(MAX_SHOWN) {
        match &quoted {
            Some(quoted) => out.push_str(&report(diag, &path, quoted, color)),
            None => out.push_str(&plain(diag, &path)),
        }
    }
    if let Some(rest) = diagnostics.len().checked_sub(MAX_SHOWN).filter(|&rest| rest > 0) {
        let _ = writeln!(out, "…and {rest} more");
    }
    out
}

fn headline(diag: &Diagnostic) -> String {
    format!("{}  [{}]", escape(&diag.message), diag.code.as_str())
}

/// The file's text as ariadne quotes it: without a leading BOM, which is neither shown nor counted as a column (D-75),
/// and after [`sanitize_source`].
struct Quoted<'a> {
    /// The text after the BOM.
    text: &'a str,
    /// Bytes skipped before `text`.
    skip: usize,
    clean: String,
}

impl<'a> Quoted<'a> {
    fn new(file: &'a str) -> Self {
        let text = file.strip_prefix('\u{feff}').unwrap_or(file);
        Quoted {
            text,
            skip: file.len() - text.len(),
            clean: sanitize_source(text),
        }
    }

    /// The file byte span as a character range of `text`, as ariadne counts with `IndexType::Char`.
    fn chars(&self, span: Span) -> Range<usize> {
        let start = char_count(self.text, span.start.saturating_sub(self.skip));
        start..char_count(self.text, span.end.saturating_sub(self.skip)).max(start)
    }
}

/// One diagnostic through ariadne; `path` is already escaped.
fn report(diag: &Diagnostic, path: &str, quoted: &Quoted<'_>, color: bool) -> String {
    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
    };
    let at = |span: Span| (path, quoted.chars(span));
    let mut builder = Report::build(kind, at(diag.span))
        .with_config(Config::default().with_color(color).with_index_type(IndexType::Char))
        .with_message(headline(diag))
        .with_label(
            AriadneLabel::new(at(diag.span))
                .with_message(PRIMARY_LABEL)
                .with_order(-1),
        );
    for label in &diag.labels {
        builder.add_label(AriadneLabel::new(at(label.span)).with_message(escape(&label.text)));
    }
    for note in &diag.notes {
        builder.add_note(escape(note));
    }
    if let Some(help) = &diag.help {
        builder.add_help(escape(help));
    }
    let mut buf = Vec::new();
    match builder
        .finish()
        .write((path, Source::from(quoted.clean.as_str())), &mut buf)
    {
        Ok(()) => String::from_utf8_lossy(&buf).into_owned(),
        // Writing into a `Vec` only fails if ariadne can't place a span; the message still has to reach the learner.
        Err(_) => plain(diag, path),
    }
}

/// A diagnostic without source lines, for a file that couldn't be read. `path` is already escaped.
fn plain(diag: &Diagnostic, path: &str) -> String {
    let kind = match diag.severity {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
    };
    let mut out = format!("{kind}: {}\n   ╭─[ {path} ]\n", headline(diag));
    for note in &diag.notes {
        let _ = writeln!(out, "   │ Note: {}", escape(note));
    }
    if let Some(help) = &diag.help {
        let _ = writeln!(out, "   │ Help: {}", escape(help));
    }
    out.push_str("───╯\n");
    out
}

/// Characters that start before byte `offset`.
fn char_count(text: &str, offset: usize) -> usize {
    text.char_indices().take_while(|&(i, _)| i < offset).count()
}

/// Whether `c` must not reach a terminal as is (R-CLI-17): C0/C1 controls other than tab and newline, DEL, the bidi
/// controls, and the separators ariadne would start a new line at although Velme doesn't (R-SYN-02).
fn is_unsafe(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}'
        )
}

/// Escapes unsafe characters in a message as `\u{..}` (R-CLI-17).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if is_unsafe(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// Escapes, in serialized JSON, the characters [`escape`] makes visible, as `\uXXXX` (R-CLI-17); the decoded value is
/// unchanged. serde_json already escapes the rest of C0, and outside strings it writes only ASCII punctuation, space and
/// newline, so every character replaced here is inside a string.
pub fn escape_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if is_unsafe(c) {
            let _ = write!(out, "\\u{:04x}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// Replaces each unsafe character in quoted source with one `�`, keeping character offsets; a `\r` directly before
/// `\n` stays, so ariadne's lines are Velme's lines.
fn sanitize_source(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let crlf = c == '\r' && chars.peek() == Some(&'\n');
        out.push(if is_unsafe(c) && !crlf { '\u{fffd}' } else { c });
    }
    out
}

/// One diagnostic in the JSON shape of `tooling/40` R-CLI-08 (D-68). Fields are only ever added (R-CMP-15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JsonDiagnostic {
    /// The stable code, `"VL0101"`.
    pub code: Code,
    /// `"error"` or `"warning"`.
    pub severity: Severity,
    /// The same text as the human headline, without the code.
    pub message: String,
    /// The project-relative path with `/` separators (R-CLI-19).
    pub file: String,
    /// The primary span.
    pub span: JsonSpan,
    /// Secondary spans.
    pub labels: Vec<JsonLabel>,
    /// Extra detail.
    pub notes: Vec<String>,
    /// A suggested fix, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// A span with byte offsets for tools and a 1-based line and column for people; the column counts Unicode scalar
/// values (D-68).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JsonSpan {
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset one past the last byte.
    pub end: usize,
    /// 1-based line of `start`.
    pub line: usize,
    /// 1-based column of `start`, in Unicode scalar values.
    pub column: usize,
}

/// A secondary label in JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JsonLabel {
    /// Where the label points.
    pub span: JsonSpan,
    /// What the label says there.
    pub text: String,
}

/// Where each line of one file's text starts, so that many spans are placed without rescanning the file (D-68).
#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
    text: &'a str,
    /// Byte offset of each line's first character; line 1 starts after a leading BOM (D-75).
    starts: Vec<usize>,
    /// Whether each line is ASCII, where a column is a byte count.
    ascii: Vec<bool>,
}

impl<'a> LineIndex<'a> {
    /// Indexes `text`, which is empty when the file couldn't be read.
    pub fn new(text: &'a str) -> Self {
        let first = if text.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let starts: Vec<usize> = std::iter::once(first)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let ends = starts.iter().skip(1).copied().chain([text.len()]);
        let ascii = starts
            .iter()
            .zip(ends)
            .map(|(&start, end)| text.get(start..end).is_some_and(str::is_ascii))
            .collect();
        LineIndex { text, starts, ascii }
    }

    /// Locates `span`; an offset past the end is placed at the end.
    pub fn locate(&self, span: Span) -> JsonSpan {
        let offset = span.start.min(self.text.len());
        let line = self.starts.partition_point(|&start| start <= offset).max(1);
        let line_start = self.starts.get(line - 1).map_or(0, |&start| start.min(offset));
        let column = if self.ascii.get(line - 1).copied().unwrap_or(false) {
            offset - line_start
        } else {
            self.text.get(line_start..).map_or(0, |rest| {
                rest.char_indices()
                    .take_while(|&(i, _)| i < offset - line_start)
                    .count()
            })
        };
        JsonSpan {
            start: span.start,
            end: span.end,
            line,
            column: column + 1,
        }
    }
}

impl JsonDiagnostic {
    /// The JSON form of `diag`, found in the file at `path` whose text `lines` indexes.
    pub fn new(diag: &Diagnostic, path: &str, lines: &LineIndex<'_>) -> Self {
        JsonDiagnostic {
            code: diag.code,
            severity: diag.severity,
            message: diag.message.clone(),
            file: path.to_owned(),
            span: lines.locate(diag.span),
            labels: diag
                .labels
                .iter()
                .map(|Label { span, text }| JsonLabel {
                    span: lines.locate(*span),
                    text: text.clone(),
                })
                .collect(),
            notes: diag.notes.clone(),
            help: diag.help.clone(),
        }
    }
}
