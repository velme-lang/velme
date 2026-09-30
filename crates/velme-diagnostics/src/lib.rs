//! Velme diagnostics: the `VLnnnn` code enum, spans and the `Diagnostic` type (`compiler/20` §5, `reference/90` §2).
#![forbid(unsafe_code)]

use serde::{Serialize, Serializer};

pub mod render;

macro_rules! codes {
    ($($name:ident = $code:literal, $severity:ident;)+) => {
        /// A stable diagnostic code (`reference/90` §2, INV-10). The variant name is the code's name there.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Code {
            $(
                #[doc = concat!("`", $code, "`")]
                $name,
            )+
        }

        impl Code {
            /// Every code, in `reference/90` order.
            pub const ALL: &[Code] = &[$(Code::$name,)+];

            /// The code as written in output: `"VL0101"`.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Code::$name => $code,)+
                }
            }

            /// The code's name in `reference/90`: `"UnexpectedToken"`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Code::$name => stringify!($name),)+
                }
            }

            /// Whether this code is an error or a warning; `VL0107` is the only v0.1 warning (D-69).
            pub fn severity(self) -> Severity {
                match self {
                    $(Code::$name => Severity::$severity,)+
                }
            }
        }
    };
}

codes! {
    UnexpectedToken = "VL0101", Error;
    InconsistentIndentation = "VL0102", Error;
    TabIndentation = "VL0103", Error;
    ReservedWord = "VL0104", Error;
    UnterminatedText = "VL0105", Error;
    UnsupportedLanguageVersion = "VL0106", Error;
    LintWarning = "VL0107", Warning;
    UnknownType = "VL0201", Error;
    UnknownName = "VL0202", Error;
    DuplicateDeclaration = "VL0203", Error;
    TypeMismatch = "VL0204", Error;
    UnknownField = "VL0205", Error;
    InvalidOperandType = "VL0206", Error;
    NullableAccess = "VL0207", Error;
    RecursiveType = "VL0208", Error;
    UnknownGoal = "VL0301", Error;
    CallArityMismatch = "VL0302", Error;
    InvalidCall = "VL0303", Error;
    CallCycle = "VL0304", Error;
    BindingUsedBeforeDefinition = "VL0305", Error;
    DuplicateBinding = "VL0306", Error;
    GoalHasNoBody = "VL0307", Error;
    InvalidBudget = "VL0308", Error;
    IRSchemaInvalid = "VL0401", Error;
    IRInvalid = "VL0402", Error;
    SynthesisFailed = "VL0403", Error;
    ProviderUnavailable = "VL0404", Error;
    ProviderNotConfigured = "VL0405", Error;
    BackendFailed = "VL0406", Error;
    PlanUnclear = "VL0407", Error;
    SynthesisPending = "VL0408", Error;
    SynthesisBlocked = "VL0409", Error;
    CheckFailed = "VL0501", Error;
    ExampleFailed = "VL0502", Error;
    VerificationFailed = "VL0503", Error;
    BudgetExceeded = "VL0601", Error;
    ArithmeticError = "VL0602", Error;
    Timeout = "VL0603", Error;
    MemoryLimitExceeded = "VL0604", Error;
    CallLimitExceeded = "VL0605", Error;
    SizeLimitExceeded = "VL0606", Error;
    InternalError = "VL0607", Error;
    ArtifactUnavailable = "VL0701", Error;
    LockStale = "VL0702", Error;
    ArtifactCorrupt = "VL0703", Error;
    CapabilityDenied = "VL0801", Error;
    FileError = "VL0901", Error;
    InvalidInput = "VL0902", Error;
    GoalNotFound = "VL0903", Error;
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// How serious a diagnostic is (`compiler/20` R-CMP-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Stops `check`, `build` and `run`.
    Error,
    /// Reported, never blocking (D-69).
    Warning,
}

/// A half-open byte range `start..end` in one source file (CC-API-05). Serialized as `[start, end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset one past the last byte.
    pub end: usize,
}

impl Span {
    /// The span `start..end`.
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

impl Serialize for Span {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.start, self.end].serialize(s)
    }
}

/// A secondary span with its own text (`compiler/20` R-CMP-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// Where the label points.
    pub span: Span,
    /// What the label says there.
    pub text: String,
}

/// A user-facing problem: code, primary span, learner message and optional labels, notes and help (R-CMP-13, P-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The stable code.
    pub code: Code,
    /// Error or warning; follows the code.
    pub severity: Severity,
    /// The learner-facing message (CC-ERR-03).
    pub message: String,
    /// The primary span.
    pub span: Span,
    /// Secondary spans.
    pub labels: Vec<Label>,
    /// Extra detail; jargon goes here, not in `message` (R-CMP-14).
    pub notes: Vec<String>,
    /// A suggested fix.
    pub help: Option<Box<str>>,
    /// The file this diagnostic is about when it has no place in the source file being compiled, such as `velme.toml` or
    /// `velme.lock` (D-111); `None` when it is about that source file. Boxed so a `Diagnostic` stays small enough to return
    /// in a `Result`.
    pub file: Option<Box<String>>,
}

impl Diagnostic {
    /// A diagnostic with `code`'s severity, pointing at `span`.
    pub fn new(code: Code, span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            severity: code.severity(),
            message: message.into(),
            span,
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
            file: None,
        }
    }

    /// `VL0607`: a bug inside Velme, with the `reference/90` wording (D-74). It belongs to no span.
    pub fn internal_error() -> Self {
        Diagnostic::new(
            Code::InternalError,
            Span::default(),
            concat!(
                "Something went wrong inside Velme. Please report it: ",
                env!("CARGO_PKG_REPOSITORY"),
                "/issues."
            ),
        )
    }

    /// Adds a secondary label.
    pub fn with_label(mut self, span: Span, text: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            text: text.into(),
        });
        self
    }

    /// Adds a note.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Sets the help line.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into().into_boxed_str());
        self
    }

    /// Names the file this diagnostic is about, when that is not the source file (D-111).
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(Box::new(file.into()));
        self
    }

    /// Whether this diagnostic stops compilation.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Whether `c` is a Unicode bidi control: it can make displayed and read order differ ("Trojan Source"). The one
/// list for the R-SYN-22 lint and for escaping (R-CLI-17).
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Orders one file's diagnostics by start offset, then code, keeping the emit order for ties (R-CMP-16, INV-3).
pub fn sort(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_key(|d| (d.span.start, d.code));
}

/// The candidate closest to `word`, for a "did you mean" help line: at most two edits away, and fewer edits than the
/// candidate has letters. Ties go to the alphabetically first.
pub fn closest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|c| (edit_distance(word, c), c))
        .filter(|&(d, c)| d <= 2 && d < c.chars().count())
        .min()
        .map(|(_, c)| c)
}

/// The "did you mean `x`?" help line for the `closest` candidate to `word`, if there is one.
pub fn did_you_mean<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    closest(word, candidates).map(|c| format!("did you mean `{c}`?"))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = i;
        let mut prev = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let above = row.get(j + 1).copied().unwrap_or(0);
            let next = (diag + usize::from(ca != cb)).min(prev + 1).min(above + 1);
            diag = above;
            if let Some(slot) = row.get_mut(j + 1) {
                *slot = next;
            }
            prev = next;
        }
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
    }
    row.last().copied().unwrap_or(0)
}
