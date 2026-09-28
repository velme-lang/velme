//! Source files (`language/10` R-SYN-01, `compiler/20` §6).

use velme_diagnostics::{Code, Diagnostic, Span};

/// One `.velme` file: its project-relative path and its text, without a leading BOM (R-SYN-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Project-relative path with `/` separators (`tooling/40` R-CLI-19).
    pub path: String,
    /// The file's text. Every `Span` is a byte range into it.
    pub text: String,
}

impl SourceFile {
    /// A source file from text already known to be valid; a leading BOM is dropped.
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let text = match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_owned(),
            None => text,
        };
        SourceFile {
            path: path.into(),
            text,
        }
    }

    /// Decodes raw file bytes; bytes that aren't UTF-8 are `VL0901` (R-SYN-01, AC-SYN-14).
    pub fn from_bytes(path: impl Into<String>, bytes: Vec<u8>) -> Result<Self, Diagnostic> {
        let path = path.into();
        match String::from_utf8(bytes) {
            Ok(text) => Ok(SourceFile::new(path, text)),
            Err(err) => {
                let at = err.utf8_error().valid_up_to();
                Err(Diagnostic::new(
                    Code::FileError,
                    Span::new(at, at),
                    format!("I couldn't read `{path}`: it isn't saved as UTF-8 text."),
                )
                .with_help("save the file with UTF-8 encoding"))
            }
        }
    }
}
