//! Parser (`language/10` §4–§5): tokens → [`Program`].
//!
//! The token stream is cut into top-level chunks, one declaration each, using the layout tokens. Each chunk is
//! parsed on its own, so an error in one declaration never hides the next (R-SYN-17); inside a declaration the parser
//! recovers at line boundaries. Rules that are easier to state on a parsed shape (block order, `T??`, chained
//! comparisons, mixed arguments, number ranges, `result` placement) are checked with `validate`, so the grammar stays
//! permissive and the message can say exactly what is wrong.

use chumsky::error::{RichPattern, RichReason};
use chumsky::input::ValueInput;
use chumsky::prelude::*;
use velme_diagnostics::{Code, Diagnostic, Span, closest};

use crate::BuiltinType;
use crate::ast::{
    Arg, BaseType, BinaryOp, Binding, Budget, BudgetItem, CallArg, CallBlock, CheckBlock, Decl, Example, ExamplesBlock,
    Expr, ExprKind, FailedDecl, Field, FieldValue, GoalDecl, Header, Ident, Literal, LiteralKind, Number, Param, Path,
    Plan, PlanForm, Program, Quantifier, TypeDecl, TypeExpr, UnaryOp, UnitLit, Version,
};
use crate::token::{Punct, Token, TokenKind as Tok};
use crate::{Keyword, LANGUAGE_VERSION, SourceFile, lex};

type Error<'t> = Rich<'t, Tok>;
type Extra<'t> = extra::Err<Error<'t>>;

/// Deepest nesting of brackets and prefix forms on one line (D-71). Deeper input is rejected before parsing, so the
/// recursive parser can't overflow the stack (R-SYN-19).
const MAX_NESTING: usize = 32;

/// Most infix operators and `.` chained on one line (D-71). The parse is iterative, but the tree it builds is as deep
/// as the chain, and dropping or walking it recurses.
const MAX_CHAIN: usize = 256;

/// Separates a custom error's message from its help line inside a `Rich::custom` string.
const HELP_SEP: char = '\u{1F}';

/// The largest `Number` coefficient, `2^96 - 1` (D-36).
const MAX_COEFFICIENT: &str = "79228162514264337593543950335";

/// Most digits a `Number` keeps after the `.` (R-SYN-03).
const MAX_FRACTION_DIGITS: usize = 28;

const BLOCK_ORDER_HELP: &str = "a goal's parts go in this order: `budget`, `call:`, `plan:`, `check:`, `examples:`";

/// Parses a whole file (`language/10` §4). Always returns a program; every problem, including the lexer's, is a
/// diagnostic, one per root cause (D-76), sorted by position then code (R-SYN-17, R-SYN-19, R-CMP-16).
pub fn parse(file: &SourceFile) -> (Program, Vec<Diagnostic>) {
    let text = file.text.as_str();
    let (tokens, mut diags) = lex(file);
    let tokens = parser_tokens(tokens, &mut diags);
    // Where the errors found before parsing start (the lexer's and reserved words, VL0104), sorted; and those that
    // left the tokens damaged, which excludes indentation errors (D-76).
    let mut early: Vec<usize> = diags.iter().filter(|d| d.is_error()).map(|d| d.span.start).collect();
    early.sort_unstable();
    let mut damage: Vec<usize> = diags
        .iter()
        .filter(|d| d.is_error() && d.code != Code::TabIndentation && d.code != Code::InconsistentIndentation)
        .map(|d| d.span.start)
        .collect();
    damage.sort_unstable();
    let mut program = Program {
        header: None,
        decls: Vec::new(),
        failed: Vec::new(),
        span: Span::new(0, text.len()),
    };
    let chunks = chunks(&tokens);
    for (index, chunk) in chunks.iter().copied().enumerate() {
        let next = chunks.get(index + 1).and_then(|c| c.first());
        let (item, errors) = parse_chunk(chunk, next.map(|(tok, _)| tok));
        diags.extend(
            errors
                .into_iter()
                .filter(|d| !follows_damage(text, &damage, d.span.start)),
        );
        // D-76: a declaration holding any error, the lexer's included, is excluded like one that didn't parse.
        let from = chunk.first().map_or(0, |(_, s)| s.start);
        let to = next.map_or(text.len(), |(_, s)| s.start);
        let first_at_or_after = early.partition_point(|&at| at < from);
        let clean = early.get(first_at_or_after).is_none_or(|&at| at >= to);
        match item {
            Some(Item::Header(header)) if index == 0 => {
                if clean {
                    check_header(&header, &mut diags);
                }
                program.header = Some(header);
            }
            Some(Item::Header(header)) => diags.push(
                Diagnostic::new(
                    Code::UnexpectedToken,
                    header.span,
                    "The `language:` line has to be the first line of the file.",
                )
                .with_help("move it to the top of the file"),
            ),
            Some(Item::Decl(decl)) if clean => {
                lint_names(&decl, &mut diags);
                program.decls.push(decl);
            }
            Some(Item::Decl(_)) | None => {
                if let Some(failed) = failed_decl(chunk) {
                    program.failed.push(failed);
                }
            }
        }
    }
    velme_diagnostics::sort(&mut diags);
    one_error_per_start(&mut diags);
    (program, diags)
}

/// `tooling/40` §5: keeps the first error at each span start; a second one there is a cascade from the same place.
/// Expects `diags` sorted by position.
fn one_error_per_start(diags: &mut Vec<Diagnostic>) {
    let mut last_error = None;
    diags.retain(|d| {
        if !d.is_error() {
            return true;
        }
        let first = last_error != Some(d.span.start);
        last_error = Some(d.span.start);
        first
    });
}

/// Whether a parser error at `at` follows from an earlier error on its line that left the tokens damaged: a
/// character or word Velme can't read, bad text or a reserved word (D-76). `damage` holds their sorted starts.
fn follows_damage(text: &str, damage: &[usize], at: usize) -> bool {
    let before = damage.partition_point(|&start| start <= at);
    before
        .checked_sub(1)
        .and_then(|i| damage.get(i))
        .and_then(|&start| text.get(start..at))
        // A lone `\r` ends a line too (R-SYN-02).
        .is_some_and(|between| !between.contains(['\n', '\r']))
}

/// A parsed chunk.
enum Item {
    Header(Header),
    Decl(Decl),
}

fn sp(span: SimpleSpan) -> Span {
    Span::new(span.start, span.end)
}

fn simple(span: Span) -> SimpleSpan {
    SimpleSpan::from(span.start..span.end)
}

/// Prepares the lexer's tokens for the parser. Reserved words (D-24) are `VL0104` wherever they appear, then read as
/// names so parsing continues (R-SYN-05). A `DEDENT` moves to the end of the token before it, so a block's span ends
/// at its last character rather than at the next line.
fn parser_tokens(tokens: Vec<Token>, diags: &mut Vec<Diagnostic>) -> Vec<(Tok, SimpleSpan)> {
    let mut prev_end = 0;
    tokens
        .into_iter()
        .map(|Token { kind, mut span }| {
            if kind == Tok::Dedent {
                span = Span::new(prev_end, prev_end);
            }
            prev_end = span.end;
            let kind = match kind {
                Tok::Keyword(kw) if kw.is_reserved() => {
                    diags.push(Diagnostic::new(
                        Code::ReservedWord,
                        span,
                        format!("`{kw}` is coming in a later Velme version — try another name."),
                    ));
                    Tok::Name(kw.as_str().to_owned())
                }
                kind => kind,
            };
            (kind, simple(span))
        })
        .collect()
}

/// Splits the token stream into top-level chunks: a line at indentation 0 plus the indented block under it.
fn chunks(tokens: &[(Tok, SimpleSpan)]) -> Vec<&[(Tok, SimpleSpan)]> {
    let mut out = Vec::new();
    let mut level = 0usize;
    let mut start = 0;
    let mut prev_ends_line = false;
    for (i, (tok, _)) in tokens.iter().enumerate() {
        if i > start && level == 0 && prev_ends_line && *tok != Tok::Indent {
            out.push(tokens.get(start..i).unwrap_or_default());
            start = i;
        }
        match tok {
            Tok::Indent => level += 1,
            Tok::Dedent => level = level.saturating_sub(1),
            _ => {}
        }
        prev_ends_line = matches!(tok, Tok::Newline | Tok::Dedent | Tok::BlockText(_));
    }
    if start < tokens.len() {
        out.push(tokens.get(start..).unwrap_or_default());
    }
    out
}

fn failed_decl(chunk: &[(Tok, SimpleSpan)]) -> Option<FailedDecl> {
    let (first, rest) = chunk.split_first()?;
    let name = match (&first.0, rest.first()) {
        (Tok::Keyword(Keyword::Language), _) => return None,
        (Tok::Keyword(Keyword::Type | Keyword::Goal), Some((Tok::Name(name), span))) => Some(Ident {
            name: name.clone(),
            span: sp(*span),
        }),
        _ => None,
    };
    let end = chunk.last().map_or(first.1.end, |(_, s)| s.end);
    Some(FailedDecl {
        name,
        span: Span::new(first.1.start, end),
    })
}

/// Parses one chunk into an item, or `None` and its syntax errors (the declaration is then excluded, R-SYN-17).
/// `next` is the token after the chunk, which is what the parser really meets when the chunk runs out.
fn parse_chunk(chunk: &[(Tok, SimpleSpan)], next: Option<&Tok>) -> (Option<Item>, Vec<Diagnostic>) {
    if let Some((at, message)) = too_deep(chunk) {
        let diag = Diagnostic::new(Code::UnexpectedToken, sp(at), message).with_help("split it into smaller pieces");
        return (None, vec![diag]);
    }
    let end = chunk.last().map_or(0, |(_, s)| s.end);
    let input = chunk.map(SimpleSpan::from(end..end), |(t, s)| (t, s));
    let (item, errors) = item_parser().parse(input).into_output_errors();
    if errors.is_empty() {
        return (item, Vec::new());
    }
    (None, errors.into_iter().map(|err| to_diagnostic(err, next)).collect())
}

/// The first token past [`MAX_NESTING`] levels of nesting, or past [`MAX_CHAIN`] chained operators on one line, if
/// any (D-71). Recovery recurses per indentation level, parsing per open bracket and prefix form, and dropping or
/// walking the tree per chained operator; flat forms such as `a < b and c < d` don't nest.
fn too_deep(chunk: &[(Tok, SimpleSpan)]) -> Option<(SimpleSpan, &'static str)> {
    /// One bracket level of the current line: `(`, `[`, a type's `<`, or the line itself.
    #[derive(Default)]
    struct Frame {
        /// Whether `>` closes it.
        angle: bool,
        /// `if` and quantifiers: their right side runs to the end of the level or the next `,`.
        open: usize,
        /// `not`s: each ends at the next `and`, `or`, `then`, `in` or `has`.
        not: usize,
        /// Prefix `-`s: each ends at the next infix operator.
        neg: usize,
        /// Infix operators and `.` since the last `,`.
        chain: usize,
    }
    let is_type = matches!(chunk.first(), Some((Tok::Keyword(Keyword::Type), _)));
    // A `type`'s lines and a `goal`'s first line are types, where `<` opens a level; elsewhere it compares.
    let mut in_type = is_type || matches!(chunk.first(), Some((Tok::Keyword(Keyword::Goal), _)));
    let mut indent = 0usize;
    let mut frames = vec![Frame::default()];
    let mut prev: Option<&Tok> = None;
    for (tok, span) in chunk {
        let after_operand = matches!(
            prev,
            Some(
                Tok::Name(_)
                    | Tok::Number(_)
                    | Tok::Unit(_)
                    | Tok::Text(_)
                    | Tok::Punct(Punct::RParen | Punct::RBracket)
                    | Tok::Keyword(Keyword::True | Keyword::False | Keyword::Nothing | Keyword::Result)
            )
        );
        let after_is = prev == Some(&Tok::Keyword(Keyword::Is));
        prev = Some(tok);
        let closes_angle = frames.len() > 1 && frames.last().is_some_and(|f| f.angle);
        match tok {
            Tok::Newline => {
                frames = vec![Frame::default()];
                in_type = is_type;
            }
            Tok::Indent => indent += 1,
            Tok::Dedent => indent = indent.saturating_sub(1),
            Tok::Punct(Punct::LParen | Punct::LBracket) => frames.push(Frame::default()),
            Tok::Punct(Punct::Lt) if in_type => frames.push(Frame {
                angle: true,
                ..Frame::default()
            }),
            Tok::Punct(Punct::RParen | Punct::RBracket) if frames.len() > 1 => {
                frames.pop();
            }
            Tok::Punct(Punct::Gt) if closes_angle => {
                frames.pop();
            }
            _ => {
                let Some(top) = frames.last_mut() else { continue };
                match tok {
                    Tok::Punct(Punct::Comma) => {
                        *top = Frame {
                            angle: top.angle,
                            ..Frame::default()
                        }
                    }
                    Tok::Keyword(Keyword::If | Keyword::Every | Keyword::Some) => top.open += 1,
                    Tok::Keyword(Keyword::Not) if !after_is => top.not += 1,
                    Tok::Punct(Punct::Minus) if !after_operand => top.neg += 1,
                    Tok::Keyword(Keyword::Then | Keyword::In | Keyword::Has) => (top.not, top.neg) = (0, 0),
                    Tok::Keyword(Keyword::And | Keyword::Or) => {
                        (top.not, top.neg) = (0, 0);
                        top.chain += 1;
                    }
                    Tok::Punct(Punct::Dot) => top.chain += 1,
                    Tok::Keyword(Keyword::Is)
                    | Tok::Punct(
                        Punct::Minus
                        | Punct::Plus
                        | Punct::Star
                        | Punct::Slash
                        | Punct::EqEq
                        | Punct::NotEq
                        | Punct::Lt
                        | Punct::LtEq
                        | Punct::Gt
                        | Punct::GtEq,
                    ) => {
                        top.neg = 0;
                        top.chain += 1;
                    }
                    _ => {}
                }
            }
        }
        let depth = indent + frames.iter().map(|f| 1 + f.open + f.not + f.neg).sum::<usize>();
        let chain = frames.iter().map(|f| f.chain).sum::<usize>();
        if depth > MAX_NESTING {
            return Some((*span, "This line nests too deeply for me to read."));
        }
        if chain > MAX_CHAIN {
            return Some((*span, "This line chains too many operators for me to read."));
        }
    }
    None
}

/// R-SYN-15, R-SYN-21: the header names `velme` and a supported version, compared as text.
fn check_header(header: &Header, diags: &mut Vec<Diagnostic>) {
    if header.language.name == "velme" && header.version.text == LANGUAGE_VERSION {
        return;
    }
    diags.push(
        Diagnostic::new(
            Code::UnsupportedLanguageVersion,
            header.language.span.to(header.version.span),
            format!(
                "This file is written for `{}/{}`, but this Velme understands `velme/{LANGUAGE_VERSION}`.",
                header.language.name, header.version.text
            ),
        )
        .with_help(format!("change the first line to `language: velme/{LANGUAGE_VERSION}`")),
    );
}

/// R-SYN-03: the exact decimal text of a `NUMBER`, or the message and help for one out of range.
fn check_number(raw: &str) -> Result<String, (&'static str, &'static str)> {
    let text: String = raw.chars().filter(|&c| c != '_').collect();
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    if frac.len() > MAX_FRACTION_DIGITS {
        return Err((
            "This number has too many decimal places.",
            "Velme numbers keep up to 28 digits after the `.`",
        ));
    }
    let coefficient = format!("{int}{frac}");
    let significant = coefficient.trim_start_matches('0');
    if (significant.len(), significant) > (MAX_COEFFICIENT.len(), MAX_COEFFICIENT) {
        return Err((
            "This number is too big.",
            "Velme numbers have at most 28 significant digits (up to 79228162514264337593543950335)",
        ));
    }
    Ok(text)
}

fn custom<'t>(span: SimpleSpan, message: impl Into<String>, help: &str) -> Error<'t> {
    Rich::custom(span, format!("{}{HELP_SEP}{help}", message.into()))
}

/// Turns a parser error into a learner-facing diagnostic (R-SYN-18): no internal token names.
fn to_diagnostic(err: Error<'_>, next: Option<&Tok>) -> Diagnostic {
    let span = sp(*err.span());
    match err.into_reason() {
        RichReason::Custom(text) => {
            let (message, help) = text.split_once(HELP_SEP).unwrap_or((&text, ""));
            let diag = Diagnostic::new(Code::UnexpectedToken, span, message);
            if help.is_empty() { diag } else { diag.with_help(help) }
        }
        RichReason::ExpectedFound { expected, found } => {
            let found = found.as_deref();
            if found == Some(&Tok::Indent) {
                return Diagnostic::new(
                    Code::InconsistentIndentation,
                    span,
                    "This line is indented more than I expected.",
                )
                .with_help("line it up with the line above, or end the line above with `:` to start a block");
            }
            let mut wanted: Vec<String> = expected.iter().filter_map(describe).collect();
            wanted.sort();
            wanted.dedup();
            let found_text = found
                .or(next)
                .map_or_else(|| "the end of the file".to_owned(), ToString::to_string);
            let message = match wanted.as_slice() {
                [] => format!("I didn't expect {found_text} here."),
                [.., _] => format!(
                    "I didn't expect {found_text} here — I was looking for {}.",
                    join_or(&wanted)
                ),
            };
            let diag = Diagnostic::new(Code::UnexpectedToken, span, message);
            if matches!(found, Some(Tok::Keyword(Keyword::If | Keyword::Every | Keyword::Some)))
                && wanted.iter().any(|w| w == "an expression")
            {
                // §4.1: an `if` or quantifier inside an operand needs parentheses.
                return diag.with_help("put it in parentheses, like `a and (if b then c)`");
            }
            match did_you_mean(found, &expected) {
                Some(word) => diag.with_help(format!("did you mean `{word}`?")),
                None => diag,
            }
        }
    }
}

fn describe(pattern: &RichPattern<'_, Tok>) -> Option<String> {
    match pattern {
        RichPattern::Token(tok) => Some(tok.to_string()),
        RichPattern::Label(label) => Some(label.to_string()),
        RichPattern::Identifier(word) => Some(format!("`{word}`")),
        // A chunk ends where its declaration does, so "end of input" is never what the learner should write.
        RichPattern::EndOfInput | RichPattern::Any | RichPattern::SomethingElse => None,
        // `RichPattern` is `#[non_exhaustive]`; a pattern chumsky adds later reads as nothing.
        _ => None,
    }
}

/// `a`, `a or b`, `a, b or c`.
fn join_or(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} or {last}", init.join(", ")),
    }
}

/// A keyword the parser expected that is close to the name it found (`chek` → `check`).
fn did_you_mean(found: Option<&Tok>, expected: &[RichPattern<'_, Tok>]) -> Option<&'static str> {
    let Some(Tok::Name(word)) = found else { return None };
    let keywords = expected.iter().filter_map(|p| match p {
        RichPattern::Token(tok) => match &**tok {
            Tok::Keyword(kw) => Some(kw.as_str()),
            _ => None,
        },
        _ => None,
    });
    closest(word, keywords)
}

// ---- naming lint (R-SYN-06) ----

fn lint_names(decl: &Decl, diags: &mut Vec<Diagnostic>) {
    match decl {
        Decl::Type(ty) => {
            lint_pascal(&ty.name, "type", diags);
            for field in &ty.fields {
                lint_snake(&field.name, "field", diags);
            }
        }
        Decl::Goal(goal) => {
            lint_pascal(&goal.name, "goal", diags);
            for param in &goal.params {
                lint_snake(&param.name, "parameter", diags);
            }
            for binding in goal.call.iter().flat_map(|c| &c.bindings) {
                lint_snake(&binding.name, "binding", diags);
            }
        }
    }
}

fn lint_pascal(ident: &Ident, what: &str, diags: &mut Vec<Diagnostic>) {
    let suggestion: String = ident
        .name
        .split('_')
        .flat_map(|part| {
            let mut chars = part.chars();
            chars.next().map(|c| c.to_ascii_uppercase()).into_iter().chain(chars)
        })
        .collect();
    lint_style(ident, suggestion, format!("{what} names use PascalCase"), diags);
}

fn lint_snake(ident: &Ident, what: &str, diags: &mut Vec<Diagnostic>) {
    let mut suggestion = String::new();
    let mut prev_lower = false;
    for c in ident.name.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            suggestion.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        suggestion.push(c.to_ascii_lowercase());
    }
    lint_style(ident, suggestion, format!("{what} names use snake_case"), diags);
}

fn lint_style(ident: &Ident, suggestion: String, rule: String, diags: &mut Vec<Diagnostic>) {
    // A reserved word already has its own error (VL0104); a style hint on it would be noise.
    if suggestion == ident.name || suggestion.is_empty() || Keyword::from_word(&ident.name).is_some() {
        return;
    }
    diags.push(
        Diagnostic::new(
            Code::LintWarning,
            ident.span,
            format!("`{}` works, but Velme style writes it `{suggestion}`.", ident.name),
        )
        .with_help(rule),
    );
}

// ---- grammar (§4) ----

/// The token input every grammar function is generic over.
trait TokenInput<'t>: ValueInput<'t, Token = Tok, Span = SimpleSpan> {}
impl<'t, I: ValueInput<'t, Token = Tok, Span = SimpleSpan>> TokenInput<'t> for I {}

fn punct<'t, I: TokenInput<'t>>(p: Punct) -> impl Parser<'t, I, Tok, Extra<'t>> + Clone {
    just(Tok::Punct(p))
}

fn kw<'t, I: TokenInput<'t>>(k: Keyword) -> impl Parser<'t, I, Tok, Extra<'t>> + Clone {
    just(Tok::Keyword(k))
}

fn newline<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Tok, Extra<'t>> + Clone {
    just(Tok::Newline)
}

fn name<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Ident, Extra<'t>> + Clone {
    select! { Tok::Name(name) = e => Ident { name, span: sp(e.span()) } }.labelled("a name")
}

/// `name` or `result`, as a binding target or the start of a call argument (R-GOAL-23).
fn name_or_result<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Ident, Extra<'t>> + Clone {
    name().or(kw(Keyword::Result).map_with(|_, e| Ident {
        name: Keyword::Result.as_str().to_owned(),
        span: sp(e.span()),
    }))
}

/// A `NUMBER`, checked and normalized (R-SYN-03).
fn number<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Number, Extra<'t>> + Clone {
    select! { Tok::Number(raw) => raw }
        .validate(|raw, e, emitter| {
            let text = check_number(&raw).unwrap_or_else(|(message, help)| {
                emitter.emit(custom(e.span(), message, help));
                raw.replace('_', "")
            });
            Number {
                text,
                span: sp(e.span()),
            }
        })
        .labelled("a number")
}

/// Comma-separated items inside `open … close`, trailing comma allowed.
fn list_of<'t, I: TokenInput<'t>, O>(
    item: impl Parser<'t, I, O, Extra<'t>> + Clone,
    open: Punct,
    close: Punct,
) -> impl Parser<'t, I, Vec<O>, Extra<'t>> + Clone {
    item.separated_by(punct(Punct::Comma))
        .allow_trailing()
        .collect()
        .delimited_by(punct(open), punct(close))
}

/// Skips a bad line and any block indented under it: the line-level recovery (R-SYN-17).
fn skip_lines<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, (), Extra<'t>> + Clone {
    let line_end = select! { Tok::Newline => (), Tok::BlockText(_) => () };
    let line = any()
        .filter(|t: &Tok| !matches!(t, Tok::Newline | Tok::Indent | Tok::Dedent | Tok::BlockText(_)))
        .repeated()
        .then(line_end)
        .ignored();
    recursive(|lines| {
        let nested = lines
            .repeated()
            .delimited_by(just(Tok::Indent), just(Tok::Dedent))
            .ignored();
        choice((line.then(nested.clone().or_not()).ignored(), nested))
    })
}

/// `INDENT item { item } DEDENT`, recovering at each bad line.
fn indented<'t, I: TokenInput<'t>, O>(
    item: impl Parser<'t, I, O, Extra<'t>> + Clone,
) -> impl Parser<'t, I, Vec<O>, Extra<'t>> + Clone {
    item.map(Some)
        .recover_with(via_parser(skip_lines().map(|()| None)))
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just(Tok::Indent), just(Tok::Dedent))
        .map(|items| items.into_iter().flatten().collect())
}

/// `keyword ":" NEWLINE INDENT item { item } DEDENT`: the `call:`, `check:` and `examples:` blocks.
fn keyword_block<'t, I: TokenInput<'t>, O>(
    keyword: Keyword,
    item: impl Parser<'t, I, O, Extra<'t>> + Clone,
) -> impl Parser<'t, I, Vec<O>, Extra<'t>> + Clone {
    kw(keyword)
        .ignore_then(punct(Punct::Colon))
        .ignore_then(newline())
        .ignore_then(indented(item))
}

fn item_parser<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Item, Extra<'t>> {
    let header = kw(Keyword::Language)
        .ignore_then(punct(Punct::Colon))
        .ignore_then(name())
        .then_ignore(punct(Punct::Slash))
        .then(
            select! { Tok::Number(text) = e => Version { text, span: sp(e.span()) } }.labelled("a version like `0.1`"),
        )
        .map_with(|(language, version), e| Header {
            language,
            version,
            span: sp(e.span()),
        })
        .then_ignore(newline());
    choice((
        header.map(Item::Header),
        type_decl().map(|d| Item::Decl(Decl::Type(d))),
        goal_decl().map(|d| Item::Decl(Decl::Goal(Box::new(d)))),
    ))
    .then_ignore(end())
}

fn type_expr<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, TypeExpr, Extra<'t>> + Clone {
    recursive(|ty| {
        let base = name()
            .then(ty.delimited_by(punct(Punct::Lt), punct(Punct::Gt)).or_not())
            .validate(|(name, element), e, emitter| match element {
                None => BaseType::Named { name },
                Some(element) => {
                    if BuiltinType::from_name(&name.name) != Some(BuiltinType::List) {
                        emitter.emit(custom(
                            e.span(),
                            format!("`{}` doesn't take a type in `< >` — only `List` does.", name.name),
                            "write `List<…>` for a list, or drop the `< >`",
                        ));
                    }
                    BaseType::List {
                        element: Box::new(element),
                    }
                }
            });
        base.then(
            punct(Punct::Question)
                .map_with(|_, e| e.span())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .validate(|(base, marks), e, emitter| {
            if let Some(&second) = marks.get(1) {
                emitter.emit(custom(
                    second,
                    "A type can be marked optional with `?` only once.",
                    "write a single `?`",
                ));
            }
            TypeExpr {
                base,
                optional: !marks.is_empty(),
                span: sp(e.span()),
            }
        })
    })
    .labelled("a type")
}

fn type_decl<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, TypeDecl, Extra<'t>> + Clone {
    let field = name()
        .then_ignore(punct(Punct::Colon))
        .then(type_expr())
        .map_with(|(name, ty), e| Field {
            name,
            ty,
            span: sp(e.span()),
        })
        .then_ignore(newline());
    kw(Keyword::Type)
        .ignore_then(name())
        .then_ignore(punct(Punct::Colon))
        .then_ignore(newline())
        .then(indented(field))
        .map_with(|(name, fields), e| TypeDecl {
            name,
            fields,
            span: sp(e.span()),
        })
}

/// One goal body block, with the span of its keyword (for order errors).
enum Block {
    Budget(Budget),
    Call(CallBlock),
    Plan(Plan),
    Check(CheckBlock),
    Examples(ExamplesBlock),
}

impl Block {
    /// Position in the required order (R-SYN-14) and the block's word.
    fn rank(&self) -> (usize, &'static str) {
        match self {
            Block::Budget(_) => (0, "budget"),
            Block::Call(_) => (1, "call:"),
            Block::Plan(_) => (2, "plan:"),
            Block::Check(_) => (3, "check:"),
            Block::Examples(_) => (4, "examples:"),
        }
    }
}

fn goal_decl<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, GoalDecl, Extra<'t>> + Clone {
    let param = name()
        .then_ignore(punct(Punct::Colon))
        .then(type_expr())
        .map_with(|(name, ty), e| Param {
            name,
            ty,
            span: sp(e.span()),
        });
    let block = choice((
        budget_line().map(Block::Budget),
        call_block().map(Block::Call),
        plan_block().map(Block::Plan),
        check_block().map(Block::Check),
        examples_block().map(Block::Examples),
    ))
    .map_with(|block, e| {
        let span: SimpleSpan = e.span();
        (block, span.start)
    });
    kw(Keyword::Goal)
        .ignore_then(name())
        .then(list_of(param, Punct::LParen, Punct::RParen))
        .then_ignore(punct(Punct::Arrow))
        .then(type_expr())
        .then_ignore(punct(Punct::Colon))
        .then_ignore(newline())
        .then(indented(block))
        .validate(|(((name, params), output), blocks), e, emitter| {
            let mut goal = GoalDecl {
                name,
                params,
                output,
                budget: None,
                call: None,
                plan: None,
                check: None,
                examples: None,
                span: sp(e.span()),
            };
            let mut furthest: Option<(usize, &str)> = None;
            for (block, start) in blocks {
                let (rank, word) = block.rank();
                let at = SimpleSpan::from(start..start + word.trim_end_matches(':').len());
                let taken = match &block {
                    Block::Budget(_) => goal.budget.is_some(),
                    Block::Call(_) => goal.call.is_some(),
                    Block::Plan(_) => goal.plan.is_some(),
                    Block::Check(_) => goal.check.is_some(),
                    Block::Examples(_) => goal.examples.is_some(),
                };
                if taken {
                    emitter.emit(custom(
                        at,
                        format!("This goal already has a `{word}` part."),
                        BLOCK_ORDER_HELP,
                    ));
                    continue;
                }
                match furthest {
                    Some((last_rank, last_word)) if rank < last_rank => {
                        emitter.emit(custom(
                            at,
                            format!("`{word}` has to come before `{last_word}`."),
                            BLOCK_ORDER_HELP,
                        ));
                    }
                    _ => furthest = Some((rank, word)),
                }
                match block {
                    Block::Budget(b) => goal.budget = Some(b),
                    Block::Call(c) => goal.call = Some(c),
                    Block::Plan(p) => goal.plan = Some(p),
                    Block::Check(c) => goal.check = Some(c),
                    Block::Examples(x) => goal.examples = Some(x),
                }
            }
            goal
        })
}

fn budget_line<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Budget, Extra<'t>> + Clone {
    let unit = select! { Tok::Unit(unit) = e => UnitLit { unit, span: sp(e.span()) } };
    let item = name()
        .then_ignore(punct(Punct::Assign))
        .then(number())
        .then(unit.or_not())
        .map_with(|((name, value), unit), e| BudgetItem {
            name,
            value,
            unit,
            span: sp(e.span()),
        });
    kw(Keyword::Budget)
        .ignore_then(item.repeated().at_least(1).collect())
        .map_with(|items, e| Budget {
            items,
            span: sp(e.span()),
        })
        .then_ignore(newline())
}

fn call_block<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, CallBlock, Extra<'t>> + Clone {
    let path = name_or_result()
        .then(punct(Punct::Dot).ignore_then(name()).repeated().collect::<Vec<_>>())
        .validate(|(first, rest), e, emitter| {
            if first.name == Keyword::Result.as_str() {
                emitter.emit(custom(
                    simple(first.span),
                    "`result` can only name the last binding.",
                    "`result` doesn't exist yet while the calls run — pass a parameter or an earlier binding",
                ));
            }
            Path {
                segments: std::iter::once(first).chain(rest).collect(),
                span: sp(e.span()),
            }
        });
    let arg = choice((literal().map(CallArg::Literal), path.map(CallArg::Path)));
    let binding = name_or_result()
        .then_ignore(punct(Punct::Assign))
        .then(name())
        .then(list_of(arg, Punct::LParen, Punct::RParen))
        .map_with(|((name, callee), args), e| Binding {
            name,
            callee,
            args,
            span: sp(e.span()),
        })
        .then_ignore(newline());
    keyword_block(Keyword::Call, binding).validate(|bindings: Vec<Binding>, e, emitter| {
        let early = bindings.iter().rev().skip(1);
        for binding in early.filter(|b| b.name.name == Keyword::Result.as_str()) {
            emitter.emit(custom(
                simple(binding.name.span),
                "`result` can only name the last binding.",
                "rename this binding, or move it to the end of `call:`",
            ));
        }
        CallBlock {
            bindings,
            span: sp(e.span()),
        }
    })
}

fn plan_block<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Plan, Extra<'t>> + Clone {
    let inline = select! { Tok::Text(text) = e => Plan { text, form: PlanForm::Inline, span: sp(e.span()) } }
        .labelled("some text in quotes")
        .then_ignore(newline());
    let block = punct(Punct::Pipe)
        .ignore_then(newline())
        .ignore_then(select! { Tok::BlockText(text) = e => Plan { text, form: PlanForm::Block, span: sp(e.span()) } });
    kw(Keyword::Plan)
        .ignore_then(punct(Punct::Colon))
        .ignore_then(choice((inline, block)))
}

fn check_block<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, CheckBlock, Extra<'t>> + Clone {
    let item = punct(Punct::Minus).ignore_then(expr()).then_ignore(newline());
    keyword_block(Keyword::Check, item).map_with(|items, e| CheckBlock {
        items,
        span: sp(e.span()),
    })
}

fn examples_block<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, ExamplesBlock, Extra<'t>> + Clone {
    let item = punct(Punct::Minus)
        .ignore_then(
            name()
                .then(list_of(literal(), Punct::LParen, Punct::RParen))
                .then_ignore(punct(Punct::EqEq))
                .then(literal())
                .map_with(|((goal, args), expected), e| Example {
                    goal,
                    args,
                    expected,
                    span: sp(e.span()),
                }),
        )
        .then_ignore(newline());
    keyword_block(Keyword::Examples, item).map_with(|items, e| ExamplesBlock {
        items,
        span: sp(e.span()),
    })
}

/// `literal` (§4): values in examples and call arguments.
fn literal<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Literal, Extra<'t>> + Clone {
    recursive(|lit| {
        let number = punct(Punct::Minus)
            .or_not()
            .then(number())
            .map(|(minus, n)| LiteralKind::Number {
                text: if minus.is_some() {
                    format!("-{}", n.text)
                } else {
                    n.text
                },
            });
        let simple = select! {
            Tok::Text(value) => LiteralKind::Text { value },
            Tok::Keyword(Keyword::True) => LiteralKind::Bool { value: true },
            Tok::Keyword(Keyword::False) => LiteralKind::Bool { value: false },
            Tok::Keyword(Keyword::Nothing) => LiteralKind::Nothing,
        };
        let list = list_of(lit.clone(), Punct::LBracket, Punct::RBracket).map(|items| LiteralKind::List { items });
        let field = name()
            .then_ignore(punct(Punct::Colon))
            .then(lit)
            .map_with(|(name, value), e| FieldValue {
                name,
                value,
                span: sp(e.span()),
            });
        let record = name()
            .then(list_of(field, Punct::LParen, Punct::RParen))
            .map(|(name, fields)| LiteralKind::Record { name, fields });
        choice((number, simple, list, record)).map_with(|kind, e| Literal {
            kind,
            span: sp(e.span()),
        })
    })
    .labelled("a value")
}

fn binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr {
        span: lhs.span.to(rhs.span),
        kind: ExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    }
}

fn prefix(op: UnaryOp, at: Span, operand: Expr) -> Expr {
    Expr {
        span: at.to(operand.span),
        kind: ExprKind::Unary {
            op,
            operand: Box::new(operand),
        },
    }
}

/// What may follow the left side of a comparison (§4 `cmp_expr`).
enum CmpTail {
    Op(BinaryOp, Expr),
    IsEmpty { negated: bool, end: Span },
}

/// `expr` (§4, precedence §4.1).
fn expr<'t, I: TokenInput<'t>>() -> impl Parser<'t, I, Expr, Extra<'t>> + Clone {
    recursive(|expr| {
        let arg = name()
            .then_ignore(punct(Punct::Colon))
            .or_not()
            .then(expr.clone())
            .map_with(|(name, value), e| Arg {
                name,
                value,
                span: sp(e.span()),
            });
        let call_or_name = name()
            .then(list_of(arg, Punct::LParen, Punct::RParen).or_not())
            .validate(|(callee, args), _, emitter| match args {
                None => ExprKind::Name { name: callee.name },
                Some(args) => {
                    let first_named = args.first().is_some_and(|a| a.name.is_some());
                    if let Some(odd) = args.iter().find(|a| a.name.is_some() != first_named) {
                        emitter.emit(custom(
                            simple(odd.span),
                            "This mixes named and unnamed arguments.",
                            "name every argument to build a record, like `Player(name: \"A\", score: 3)`, or name \
                             none to call a built-in, like `sum(1, 2)`",
                        ));
                    }
                    ExprKind::Call { callee, args }
                }
            });
        let simple_kind = select! {
            Tok::Text(value) => ExprKind::Text { value },
            Tok::Keyword(Keyword::True) => ExprKind::Bool { value: true },
            Tok::Keyword(Keyword::False) => ExprKind::Bool { value: false },
            Tok::Keyword(Keyword::Nothing) => ExprKind::Nothing,
            Tok::Keyword(Keyword::Result) => ExprKind::Result,
        };
        let list = list_of(expr.clone(), Punct::LBracket, Punct::RBracket).map(|items| ExprKind::List { items });
        let atom = choice((
            number().map(|n| ExprKind::Number { text: n.text }),
            simple_kind,
            call_or_name,
            list,
        ))
        .map_with(|kind, e| Expr {
            kind,
            span: sp(e.span()),
        });
        // The span of `(x + 1)` includes its parentheses, so `(x + 1) * 2` starts at `(`.
        let parenthesized = expr
            .clone()
            .delimited_by(punct(Punct::LParen), punct(Punct::RParen))
            .map_with(|inner: Expr, e| Expr {
                span: sp(e.span()),
                ..inner
            });
        let primary = choice((atom, parenthesized));
        let postfix = primary.foldl(punct(Punct::Dot).ignore_then(name()).repeated(), |base, field| Expr {
            span: base.span.to(field.span),
            kind: ExprKind::Field {
                base: Box::new(base),
                field,
            },
        });
        let unary = recursive(|unary| {
            punct(Punct::Minus)
                .map_with(|_, e| sp(e.span()))
                .then(unary)
                .map(|(at, operand)| prefix(UnaryOp::Neg, at, operand))
                .or(postfix.clone())
                .labelled("an expression")
        });
        let mul_op = select! { Tok::Punct(Punct::Star) => BinaryOp::Mul, Tok::Punct(Punct::Slash) => BinaryOp::Div };
        let mul = unary
            .clone()
            .foldl(mul_op.then(unary).repeated(), |l, (op, r)| binary(op, l, r));
        let add_op = select! { Tok::Punct(Punct::Plus) => BinaryOp::Add, Tok::Punct(Punct::Minus) => BinaryOp::Sub };
        let add = mul
            .clone()
            .foldl(add_op.then(mul).repeated(), |l, (op, r)| binary(op, l, r));
        let cmp_op = select! {
            Tok::Punct(Punct::EqEq) => BinaryOp::Eq,
            Tok::Punct(Punct::NotEq) => BinaryOp::NotEq,
            Tok::Punct(Punct::Lt) => BinaryOp::Lt,
            Tok::Punct(Punct::LtEq) => BinaryOp::LtEq,
            Tok::Punct(Punct::Gt) => BinaryOp::Gt,
            Tok::Punct(Punct::GtEq) => BinaryOp::GtEq,
        };
        let tail = choice((
            cmp_op
                .map_with(|op, e| (op, e.span()))
                .then(add.clone())
                .map(|((op, at), rhs)| (at, CmpTail::Op(op, rhs))),
            kw(Keyword::Is)
                .ignore_then(kw(Keyword::Not).or_not())
                .then_ignore(kw(Keyword::Empty))
                .map_with(|not, e| {
                    let tail = CmpTail::IsEmpty {
                        negated: not.is_some(),
                        end: sp(e.span()),
                    };
                    (e.span(), tail)
                }),
        ));
        let cmp = add
            .then(tail.repeated().collect::<Vec<_>>())
            .validate(|(first, tails), _, emitter| {
                if let Some(&(at, _)) = tails.get(1) {
                    emitter.emit(custom(
                        at,
                        "Comparisons can't be chained like this.",
                        "join two comparisons with `and`, like `a < b and b < c`",
                    ));
                }
                tails.into_iter().fold(first, |lhs, (_, tail)| match tail {
                    CmpTail::Op(op, rhs) => binary(op, lhs, rhs),
                    CmpTail::IsEmpty { negated, end } => Expr {
                        span: lhs.span.to(end),
                        kind: ExprKind::IsEmpty {
                            operand: Box::new(lhs),
                            negated,
                        },
                    },
                })
            });
        let not = recursive(|not| {
            kw(Keyword::Not)
                .map_with(|_, e| sp(e.span()))
                .then(not)
                .map(|(at, operand)| prefix(UnaryOp::Not, at, operand))
                .or(cmp)
                .labelled("an expression")
        });
        let and = not.clone().foldl(kw(Keyword::And).ignore_then(not).repeated(), |l, r| {
            binary(BinaryOp::And, l, r)
        });
        let or = and.clone().foldl(kw(Keyword::Or).ignore_then(and).repeated(), |l, r| {
            binary(BinaryOp::Or, l, r)
        });
        let if_expr = kw(Keyword::If)
            .map_with(|_, e| sp(e.span()))
            .then(or.clone())
            .then_ignore(kw(Keyword::Then))
            .then(expr.clone())
            .map(|((at, condition), then): ((Span, Expr), Expr)| Expr {
                span: at.to(then.span),
                kind: ExprKind::If {
                    condition: Box::new(condition),
                    then: Box::new(then),
                },
            });
        let quantifier = select! {
            Tok::Keyword(Keyword::Every) => Quantifier::Every,
            Tok::Keyword(Keyword::Some) => Quantifier::Some,
        };
        let quantified = quantifier
            .map_with(|q, e| (q, sp(e.span())))
            .then(name())
            .then_ignore(kw(Keyword::In))
            .then(postfix)
            .then_ignore(kw(Keyword::Has))
            .then(expr)
            .map(|((((quantifier, at), var), collection), body)| Expr {
                span: at.to(body.span),
                kind: ExprKind::Quantified {
                    quantifier,
                    var,
                    collection: Box::new(collection),
                    body: Box::new(body),
                },
            });
        choice((if_expr, quantified, or)).labelled("an expression").boxed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_error_per_start_skips_warnings_between() {
        let at = |start| Span::new(start, start + 1);
        let mut diags = vec![
            Diagnostic::new(Code::UnexpectedToken, at(3), "a"),
            Diagnostic::new(Code::LintWarning, at(3), "b"),
            Diagnostic::new(Code::UnknownType, at(3), "c"),
            Diagnostic::new(Code::UnexpectedToken, at(4), "d"),
        ];
        one_error_per_start(&mut diags);
        let kept: Vec<Code> = diags.iter().map(|d| d.code).collect();
        assert_eq!(kept, [Code::UnexpectedToken, Code::LintWarning, Code::UnexpectedToken]);
    }
}
