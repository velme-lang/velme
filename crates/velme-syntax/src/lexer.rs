//! Lexer and layout pass (`language/10` §2, §3): characters → tokens, with `INDENT`/`DEDENT`/`NEWLINE` and
//! `plan: |` block scalars resolved here so the parser sees a context-free token stream.

use velme_diagnostics::{Code, Diagnostic, Span};

use crate::token::{Punct, Token, TokenKind, Unit};
use crate::{Keyword, SourceFile};

/// Indentation width a tab counts for while recovering from `VL0103`.
const TAB_RECOVERY_WIDTH: usize = 4;

/// Bidi control characters that trigger the R-SYN-22 lint.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The whitespace D-21 trims from the end of each plan line: space and tab only.
const TRAILING_WHITESPACE: [char; 2] = [' ', '\t'];

/// Lexes a whole file (`language/10` §2–§3). Always returns a token stream; problems are diagnostics (R-SYN-19).
pub fn lex(file: &SourceFile) -> (Vec<Token>, Vec<Diagnostic>) {
    let (lines, lone_cr) = split_lines(&file.text);
    let mut lexer = Lexer {
        text: &file.text,
        lines,
        tokens: Vec::new(),
        diags: lone_cr.into_iter().collect(),
        indents: vec![0],
        depth: 0,
    };
    lexer.run();
    (lexer.tokens, lexer.diags)
}

/// One physical line: byte range of its content, without the `\n` or `\r\n` that ends it (R-SYN-02).
#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
}

/// Splits `text` into lines. A lone `\r` is `VL0101` (R-SYN-02); it still ends the line, so a file saved with
/// old-style line endings is reported once and otherwise read as intended. A leading BOM is skipped, and offsets stay
/// file bytes (R-SYN-01, D-75).
fn split_lines(text: &str) -> (Vec<Line>, Option<Diagnostic>) {
    let mut lines = Vec::new();
    let mut lone_cr = None;
    let mut start = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\n' => i + 1,
            '\r' if chars.peek().is_some_and(|&(_, next)| next == '\n') => {
                chars.next();
                i + 2
            }
            '\r' => {
                lone_cr.get_or_insert(i);
                i + 1
            }
            _ => continue,
        };
        lines.push(Line { start, end: i });
        start = end;
    }
    if start < text.len() {
        lines.push(Line { start, end: text.len() });
    }
    let diag = lone_cr.map(|at| {
        Diagnostic::new(
            Code::UnexpectedToken,
            Span::new(at, at + 1),
            "I didn't expect a carriage return here — lines should end with a normal line break.",
        )
        .with_help("save the file with `\\n` or `\\r\\n` line endings")
    });
    (lines, diag)
}

/// Visual width of leading whitespace; a tab (already an error) advances to the next multiple of 4.
fn width_of(ws: &str) -> usize {
    ws.chars().fold(0, |w, c| {
        if c == '\t' {
            (w / TAB_RECOVERY_WIDTH + 1) * TAB_RECOVERY_WIDTH
        } else {
            w + 1
        }
    })
}

/// A character of a word: what a name is lexed from. Non-ASCII letters are read into the word so that the whole word
/// gets one error (D-76) rather than one per letter.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `c` can start a token, or is whitespace or a comment; anything else is part of a run of unexpected
/// characters, reported once (D-76).
fn starts_token(c: char) -> bool {
    matches!(c, ' ' | '\t' | '#' | '"') || is_word_char(c) || Punct::single(c).is_some()
}

struct Lexer<'a> {
    text: &'a str,
    lines: Vec<Line>,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
    /// Indentation stack (R-SYN-09); never empty, bottom is 0.
    indents: Vec<usize>,
    /// Open `(`/`[` count; newlines and indentation are ignored while it is non-zero (R-SYN-10).
    depth: usize,
}

impl<'a> Lexer<'a> {
    fn line_text(&self, line: Line) -> &'a str {
        self.text.get(line.start..line.end).unwrap_or_default()
    }

    fn push(&mut self, kind: TokenKind, span: Span) {
        self.tokens.push(Token { kind, span });
    }

    fn error(&mut self, code: Code, span: Span, message: impl Into<String>, help: impl Into<String>) {
        self.diags.push(Diagnostic::new(code, span, message).with_help(help));
    }

    fn run(&mut self) {
        let mut i = 0;
        while let Some(&line) = self.lines.get(i) {
            i += 1;
            let text = self.line_text(line);
            let ws_len = text.len() - text.trim_start_matches([' ', '\t']).len();
            let rest = text.get(ws_len..).unwrap_or_default();
            if rest.is_empty() || rest.starts_with('#') {
                self.lint_bidi(rest, Span::new(line.start + ws_len, line.end), "comment");
                continue; // R-SYN-10: blank and comment-only lines produce no tokens.
            }
            let ws = text.get(..ws_len).unwrap_or_default();
            if let Some(tab) = ws.find('\t') {
                let at = line.start + tab;
                self.error(
                    Code::TabIndentation,
                    Span::new(at, at + 1),
                    "Please indent with spaces, not tabs.",
                    "replace the tab with spaces",
                );
            }
            let content_start = line.start + ws_len;
            // An unclosed bracket must not swallow the rest of the file: a declaration at column 0 ends it.
            if self.depth > 0 && ws_len == 0 && starts_declaration(rest) {
                self.depth = 0;
                self.push(TokenKind::Newline, Span::new(content_start, content_start));
            }
            if self.depth == 0 {
                self.layout(width_of(ws), line.start, content_start);
            }
            let first_token = self.tokens.len();
            self.lex_line(content_start, line.end);
            if self.depth == 0 {
                self.push(TokenKind::Newline, Span::new(line.end, line.end));
                if let Some(plan_col) = self.opens_block_scalar(first_token, line) {
                    i = self.block_scalar(i, plan_col, line.end);
                }
            }
        }
        let end = self.text.len();
        if self.depth > 0 {
            self.push(TokenKind::Newline, Span::new(end, end));
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.push(TokenKind::Dedent, Span::new(end, end));
        }
    }

    /// R-SYN-09: compare the line's width with the indentation stack.
    fn layout(&mut self, width: usize, line_start: usize, content_start: usize) {
        let at = Span::new(content_start, content_start);
        let top = self.indents.last().copied().unwrap_or(0);
        if width > top {
            self.indents.push(width);
            self.push(TokenKind::Indent, at);
            return;
        }
        while width < self.indents.last().copied().unwrap_or(0) {
            let outer = self
                .indents
                .len()
                .checked_sub(2)
                .and_then(|i| self.indents.get(i))
                .copied();
            if outer.is_some_and(|outer| outer < width) {
                // Between two levels: the line stays in the block it came from, at its own width, so the lines after
                // it parse as intended and the mistake is reported once (D-76).
                if let Some(top) = self.indents.last_mut() {
                    *top = width;
                }
                self.error(
                    Code::InconsistentIndentation,
                    Span::new(line_start, content_start),
                    "This line's indentation doesn't line up with the lines above it.",
                    "indent it exactly as far as the line it belongs with",
                );
                return;
            }
            self.indents.pop();
            self.push(TokenKind::Dedent, at);
        }
    }

    /// If the line just lexed ends with `plan : |`, the column of `plan` (R-SYN-12).
    fn opens_block_scalar(&self, first_token: usize, line: Line) -> Option<usize> {
        let line_tokens = self.tokens.get(first_token..)?;
        let [.., plan, colon, pipe, _newline] = line_tokens else {
            return None;
        };
        let opens = plan.kind == TokenKind::Keyword(Keyword::Plan)
            && colon.kind == TokenKind::Punct(Punct::Colon)
            && pipe.kind == TokenKind::Punct(Punct::Pipe);
        opens.then(|| width_of(self.text.get(line.start..plan.span.start).unwrap_or_default()))
    }

    /// Consumes the block scalar starting at line `first`; returns the index of the first line after it.
    /// R-SYN-12, D-21 normalization, D-67 edge cases, D-73 content column.
    fn block_scalar(&mut self, first: usize, plan_col: usize, pipe_end: usize) -> usize {
        // The content of each line, from the content column on; `None` for a blank line.
        let mut content: Vec<Option<Line>> = Vec::new();
        // D-73: the first content line's indentation.
        let mut column = None;
        let mut next = first;
        while let Some(&line) = self.lines.get(next) {
            let text = self.line_text(line);
            let ws_len = text.len() - text.trim_start_matches([' ', '\t']).len();
            if ws_len == text.len() {
                content.push(None);
                next += 1;
                continue;
            }
            let spaces = text.len() - text.trim_start_matches(' ').len();
            let width = width_of(text.get(..ws_len).unwrap_or_default());
            if width <= plan_col {
                break;
            }
            let col = *column.get_or_insert(if spaces > plan_col { spaces } else { width });
            let start = if spaces >= col {
                col
            } else if spaces < ws_len {
                // D-67: a tab before the content column is layout whitespace, so VL0103; keep the line as content.
                let at = line.start + spaces;
                self.error(
                    Code::TabIndentation,
                    Span::new(at, at + 1),
                    "Please indent with spaces, not tabs.",
                    "replace the tab with spaces",
                );
                ws_len
            } else {
                // D-73: deeper than `plan` but shallower than the text above; keep it as content.
                self.error(
                    Code::InconsistentIndentation,
                    Span::new(line.start, line.start + spaces),
                    "This line is indented less than the plan text above it.",
                    "indent it as far as the plan's first line, or no further than `plan` to end the plan",
                );
                spaces
            };
            content.push(Some(Line {
                start: line.start + start,
                end: line.end,
            }));
            next += 1;
        }
        while content.last().is_some_and(Option::is_none) {
            content.pop(); // D-67: trailing blank lines are dropped.
        }
        let lines: Vec<&str> = content
            .iter()
            .map(|line| line.map_or("", |line| self.line_text(line).trim_end_matches(TRAILING_WHITESPACE)))
            .collect();
        let span = match (content.iter().flatten().next(), content.iter().flatten().last()) {
            (Some(first), Some(last)) => Span::new(first.start, last.end),
            _ => Span::new(pipe_end, pipe_end),
        };
        let text = lines.join("\n");
        self.lint_bidi(&text, span, "text");
        self.push(TokenKind::BlockText(text), span);
        next
    }

    /// R-SYN-22: bidi control characters in text or a comment are a lint warning (D-69). `what` names which.
    fn lint_bidi(&mut self, s: &str, span: Span, what: &str) {
        if let Some(c) = s.chars().find(|&c| is_bidi_control(c)) {
            self.error(
                Code::LintWarning,
                span,
                format!(
                    "This {what} contains an invisible character (U+{:04X}) that can make it look different from \
                     what Velme reads.",
                    u32::from(c)
                ),
                "remove the invisible character",
            );
        }
    }

    /// Lexes the tokens of one line from `start` to `end` (comments dropped, R-SYN-04).
    fn lex_line(&mut self, start: usize, end: usize) {
        let mut pos = start;
        while let Some(c) = self.text.get(pos..end).and_then(|s| s.chars().next()) {
            let from = pos;
            pos += c.len_utf8();
            match c {
                ' ' | '\t' => {}
                '#' => {
                    self.lint_bidi(
                        self.text.get(from..end).unwrap_or_default(),
                        Span::new(from, end),
                        "comment",
                    );
                    return;
                }
                '"' => pos = self.text_literal(from, end),
                c if c.is_ascii_digit() => pos = self.number(from, end),
                c if is_word_char(c) => {
                    pos = self.scan_while(pos, end, is_word_char);
                    self.word(Span::new(from, pos));
                }
                c => {
                    let double = self
                        .text
                        .get(from..from + 2)
                        .and_then(|s| Punct::DOUBLE.iter().find(|(d, _)| *d == s));
                    let punct = match double {
                        Some(&(_, p)) => {
                            pos = from + 2;
                            Some(p)
                        }
                        None => Punct::single(c),
                    };
                    match punct {
                        Some(p) => {
                            match p {
                                Punct::LParen | Punct::LBracket => self.depth += 1,
                                Punct::RParen | Punct::RBracket => self.depth = self.depth.saturating_sub(1),
                                _ => {}
                            }
                            self.push(TokenKind::Punct(p), Span::new(from, pos));
                        }
                        None => {
                            // D-76: a run of characters Velme can't read is one error.
                            pos = self.scan_while(pos, end, |c| !starts_token(c));
                            let run = self.text.get(from..pos).unwrap_or_default();
                            self.diags.push(Diagnostic::new(
                                Code::UnexpectedToken,
                                Span::new(from, pos),
                                format!("I didn't expect `{run}` here."),
                            ));
                        }
                    }
                }
            }
        }
    }

    /// A word: a keyword, a name, or a name with letters outside ASCII (`VL0101`, then read as a name so parsing
    /// continues; D-76).
    fn word(&mut self, span: Span) {
        let word = self.text.get(span.start..span.end).unwrap_or_default();
        let kind = match Keyword::from_word(word) {
            Some(kw) => TokenKind::Keyword(kw),
            None => TokenKind::Name(word.to_owned()),
        };
        if !word.is_ascii() {
            self.error(
                Code::UnexpectedToken,
                span,
                format!("I can't use `{word}` as a name."),
                "names use the letters a–z and A–Z, digits and `_` in this version of Velme",
            );
        }
        self.push(kind, span);
    }

    fn scan_while(&self, mut pos: usize, end: usize, pred: impl Fn(char) -> bool) -> usize {
        while let Some(c) = self.text.get(pos..end).and_then(|s| s.chars().next()) {
            if !pred(c) {
                break;
            }
            pos += c.len_utf8();
        }
        pos
    }

    /// `NUMBER` (§2.1): digits, optional `.digits`, `_` only between digits, then a `UNIT` if one follows with no
    /// space. Returns the end offset.
    fn number(&mut self, from: usize, end: usize) -> usize {
        let is_part = |c: char| c.is_ascii_digit() || c == '_';
        let mut pos = self.scan_while(from, end, is_part);
        let rest = self.text.get(pos..end).unwrap_or_default();
        if rest.starts_with('.') && rest.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            pos = self.scan_while(pos + 1, end, is_part);
        }
        let text = self.text.get(from..pos).unwrap_or_default().to_owned();
        let bytes = text.as_bytes();
        let misplaced = bytes.iter().enumerate().any(|(i, &b)| {
            b == b'_'
                && !(i > 0
                    && bytes.get(i - 1).is_some_and(u8::is_ascii_digit)
                    && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        });
        if misplaced {
            self.error(
                Code::UnexpectedToken,
                Span::new(from, pos),
                "A `_` in a number must sit between two digits.",
                format!("write `{}`", text.replace('_', "")),
            );
        }
        self.push(TokenKind::Number(text), Span::new(from, pos));
        let word_end = self.scan_while(pos, end, is_word_char);
        match self.text.get(pos..word_end).and_then(Unit::from_word) {
            Some(unit) => {
                self.push(TokenKind::Unit(unit), Span::new(pos, word_end));
                word_end
            }
            None => pos,
        }
    }

    /// `TEXT` (§2.3, R-SYN-07). `from` is the opening quote; returns the end offset.
    fn text_literal(&mut self, from: usize, end: usize) -> usize {
        let mut value = String::new();
        let mut pos = from + 1;
        loop {
            let Some(c) = self.text.get(pos..end).and_then(|s| s.chars().next()) else {
                self.error(
                    Code::UnterminatedText,
                    Span::new(from, end),
                    "This text starts with `\"` but never ends.",
                    "add a `\"` at the end of the text",
                );
                break;
            };
            let at = pos;
            pos += c.len_utf8();
            match c {
                '"' => break,
                '\\' => pos = self.escape(at, end, &mut value),
                c => value.push(c),
            }
        }
        let span = Span::new(from, pos);
        self.lint_bidi(&value, span, "text");
        self.push(TokenKind::Text(value), span);
        pos
    }

    /// One escape starting at the backslash `at`; appends its character and returns the end offset.
    fn escape(&mut self, at: usize, end: usize, value: &mut String) -> usize {
        let rest = self.text.get(at + 1..end).unwrap_or_default();
        let Some(c) = rest.chars().next() else { return at + 1 }; // a trailing `\` leaves the text unterminated.
        let simple = match c {
            '"' => Some('"'),
            '\\' => Some('\\'),
            'n' => Some('\n'),
            't' => Some('\t'),
            _ => None,
        };
        if let Some(ch) = simple {
            value.push(ch);
            return at + 2;
        }
        if let Some(body) = rest.strip_prefix("u{") {
            let hex_len = body.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(body.len());
            let closed = body.get(hex_len..).is_some_and(|s| s.starts_with('}'));
            let len = 2 + hex_len + usize::from(closed);
            let span = Span::new(at, at + 1 + len);
            let decoded = body
                .get(..hex_len)
                .filter(|h| closed && (1..=6).contains(&h.len()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32);
            match decoded {
                Some(ch) => value.push(ch),
                None => {
                    let written = self.text.get(span.start..span.end).unwrap_or_default().to_owned();
                    self.error(
                        Code::UnexpectedToken,
                        span,
                        format!("`{written}` isn't a character Velme knows."),
                        "write `\\u{…}` with 1 to 6 hex digits naming a Unicode character",
                    );
                }
            }
            return span.end;
        }
        let span = Span::new(at, at + 1 + c.len_utf8());
        self.error(
            Code::UnexpectedToken,
            span,
            format!("`\\{c}` isn't an escape Velme knows."),
            "use `\\\"`, `\\\\`, `\\n`, `\\t` or `\\u{…}`, or write `\\\\` for a backslash",
        );
        span.end
    }
}

/// Whether a line starts a `type` or `goal` declaration (used to end an unclosed bracket).
fn starts_declaration(rest: &str) -> bool {
    [Keyword::Type, Keyword::Goal].iter().any(|kw| {
        rest.strip_prefix(kw.as_str())
            .is_some_and(|after| !after.starts_with(is_word_char))
    })
}
