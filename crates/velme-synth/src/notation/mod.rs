//! Reply notation (`compiler/22` §4.2): text is lowered before the IR trust boundary.

mod lex;
mod lower;
mod parse;

use std::collections::BTreeMap;

use velme_ir::{Node, Request};

use crate::SynthRequest;

/// The grammar and lowering version (R-SYNTH-55).
pub const GRAMMAR_VERSION: &str = "1";
pub(crate) const PROMPT: &str = include_str!("../../prompts/notation.txt");
const MAX_BYTES: usize = 64 * 1024;

/// A byte range in a notation body (R-SYNTH-59).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    /// First byte, inclusive.
    pub start: usize,
    /// Last byte, exclusive.
    pub end: usize,
}

impl Span {
    fn through(self, end: Span) -> Self {
        Self {
            start: self.start,
            end: end.end,
        }
    }
}

/// A notation rejection rule (R-SYNTH-54).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A forbidden character.
    Character,
    /// Indentation outside an open block.
    Indentation,
    /// A malformed literal.
    Literal,
    /// An unsupported statement.
    Statement,
    /// A block without a terminal return.
    Return,
    /// A reused or reserved binding.
    Binding,
    /// An unsupported operator.
    Operator,
    /// An unsupported expression.
    Expression,
    /// A form with the wrong shape.
    Shape,
    /// A Python builtin outside the catalog.
    PythonBuiltin,
    /// A list without an item type.
    ListType,
    /// A size or nesting cap.
    Limit,
    /// Other invalid syntax.
    Syntax,
}

impl Rule {
    /// The stable rule id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Character => "notation-1",
            Self::Indentation => "notation-2",
            Self::Literal => "notation-3",
            Self::Statement => "notation-4",
            Self::Return => "notation-5",
            Self::Binding => "notation-6",
            Self::Operator => "notation-7",
            Self::Expression => "notation-8",
            Self::Shape => "notation-9",
            Self::PythonBuiltin => "notation-10",
            Self::ListType => "notation-11",
            Self::Limit => "notation-12",
            Self::Syntax => "notation-13",
        }
    }

    /// The words completing "the body …".
    pub fn words(self) -> &'static str {
        match self {
            Self::Character => "held a character the notation doesn't allow",
            Self::Indentation => "was indented to no open block",
            Self::Literal => "wrote a number or Text in a form the notation doesn't have",
            Self::Statement => "had a statement the notation doesn't have",
            Self::Return => "had a block that doesn't end in `return`",
            Self::Binding => "bound a name that was already bound",
            Self::Operator => "used an operator the notation doesn't have",
            Self::Expression => "used an expression form the notation doesn't have",
            Self::Shape => "wrote a form in the wrong shape",
            Self::PythonBuiltin => "called a Python built-in that isn't one of Velme's",
            Self::ListType => "had a list whose item type Velme couldn't tell",
            Self::Limit => "was too large or nested too deeply",
            Self::Syntax => "had a syntax error",
        }
    }

    fn reject(self, span: Span) -> Rejected {
        let hint = match self {
            Self::Character => "names are ASCII letters, digits and `_`; indent with spaces; no Markdown fences",
            Self::Indentation => "indent a block's lines alike, deeper than its `if`, `elif` or `else` line",
            Self::Literal => {
                "numbers are plain decimals such as `0.05`; Text is joined with `concat`, a Number made Text with `to_text`"
            }
            Self::Statement => {
                "a loop is a comprehension, `sum`, `all`, `any`, `next`, `sorted` or `reduce`, and a name is assigned once with `=`"
            }
            Self::Return => "end each block with `return <expression>`",
            Self::Binding => "use a new name",
            Self::Operator => {
                "`a % b` is `a - b * floor(a / b)`; `a // b` is `floor(a / b)`; `**` is a product or a `reduce` over `range`"
            }
            Self::Expression => "items are found with `next(…)` and picked with comprehensions",
            Self::Shape | Self::PythonBuiltin | Self::Syntax => "",
            Self::ListType => "return the list, put it in a record field, or compare it with a typed value",
            Self::Limit => "simplify it, or the plan may be too big for one goal",
        };
        Rejected {
            rule: self,
            span: if self == Self::Limit { Span::default() } else { span },
            hint: (!hint.is_empty()).then(|| hint.to_owned()),
        }
    }
}

/// One rejection, before IR validation (R-SYNTH-59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The rule that failed.
    pub rule: Rule,
    /// Where it failed.
    pub span: Span,
    /// The fixed or per-form retry hint.
    pub hint: Option<String>,
}

impl Rejected {
    fn hint(mut self, text: &str) -> Self {
        self.hint = Some(text.to_owned());
        self
    }
}

/// Notation spans beside the IR, indexed by JSON pointer (R-SYNTH-59).
#[derive(Debug, Clone, Default)]
pub struct Spans(BTreeMap<String, (Span, Form)>);

#[derive(Debug, Clone, Default)]
pub(crate) enum Form {
    #[default]
    Other,
    NoneTest,
    Or,
    Round(usize),
}

impl Spans {
    /// The innermost node at or above `pointer`.
    pub fn at(&self, pointer: &str) -> Option<Span> {
        self.entry(pointer).map(|(span, _)| *span)
    }

    /// The node exactly at `pointer`, without searching its ancestors (R-SYNTH-59).
    pub fn exact(&self, pointer: &str) -> Option<Span> {
        self.0.get(pointer).map(|(span, _)| *span)
    }

    pub(crate) fn entry(&self, pointer: &str) -> Option<&(Span, Form)> {
        let mut at = pointer;
        loop {
            if let Some(found) = self.0.get(at) {
                return Some(found);
            }
            at = at.rsplit_once('/')?.0;
        }
    }
}

/// A lowered body and the places its nodes came from (R-SYNTH-56).
#[derive(Debug)]
pub struct Lowered {
    /// The canonical IR expression.
    pub body: Node,
    /// Source places, never serialized into the IR.
    pub spans: Spans,
}

/// Lowers one notation body deterministically, within the parser and IR caps (R-SYNTH-55).
pub fn lower(text: &str, request: &SynthRequest, ir: &Request<'_>) -> Result<Lowered, Rejected> {
    let mut lowered = lower_untyped(text, request)?;
    let untyped = velme_ir::infer_list_types(&mut lowered.body, ir);
    // Pre-order is the tie breaker, including equal spans of copied chain operands.
    if let Some(span) = untyped
        .iter()
        .filter_map(|p| lowered.spans.at(p))
        .min_by_key(|s| s.start)
    {
        return Err(Rule::ListType.reject(span));
    }
    Ok(lowered)
}

/// Lowers before contextual list inference, for checking inference and source-map coverage (R-SYNTH-55).
pub fn lower_untyped(text: &str, request: &SynthRequest) -> Result<Lowered, Rejected> {
    if text.len() > MAX_BYTES {
        return Err(Rule::Limit.reject(Span::default()));
    }
    lower::body(&parse::body(text)?, request)
}

/// A byte offset's 1-based line and scalar-value column (R-SYNTH-59).
pub fn locate(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = text.get(..offset).unwrap_or_default();
    (
        before.bytes().filter(|&c| c == b'\n').count() + 1,
        before.rsplit('\n').next().unwrap_or_default().chars().count() + 1,
    )
}

/// At most three covered lines, each cut to 160 scalar values (R-SYNTH-59).
pub fn quote(text: &str, span: Span) -> Vec<String> {
    let first = locate(text, span.start).0;
    let last = locate(text, span.end.saturating_sub(usize::from(span.end > span.start))).0;
    text.split('\n')
        .skip(first - 1)
        .take((last - first + 1).min(3))
        .map(|line| line.trim_end_matches('\r').chars().take(160).collect())
        .collect()
}
