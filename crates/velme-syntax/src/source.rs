//! Source files (`language/10` R-SYN-01, `compiler/20` §6).

use velme_diagnostics::{Code, Diagnostic, Span};

/// One `.velme` file: its project-relative path and its text as saved. A leading BOM stays in the text, so spans are
/// file byte offsets, and the lexer skips it (R-SYN-01, D-75).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Project-relative path with `/` separators (`tooling/40` R-CLI-19).
    pub path: String,
    /// The file's text. Every `Span` is a byte range into it.
    pub text: String,
}

impl SourceFile {
    /// A source file from text already known to be valid.
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        SourceFile {
            path: path.into(),
            text: text.into(),
        }
    }

    /// `VL0901` for a file that couldn't be opened or read (R-SYN-01); the OS reason goes in a note.
    pub fn unreadable(path: &str, err: &std::io::Error) -> Diagnostic {
        let diag = match err.kind() {
            std::io::ErrorKind::NotFound => {
                Diagnostic::new(Code::FileError, Span::default(), format!("I couldn't find `{path}`."))
                    .with_help("check the file name, and that you're in the folder that holds it")
            }
            _ => Diagnostic::new(Code::FileError, Span::default(), format!("I couldn't read `{path}`.")),
        };
        // It is about that file, whether or not it is the source being compiled (D-111).
        diag.with_note(err.to_string()).with_file(path)
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
