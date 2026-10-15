use std::collections::VecDeque;

use super::{Rejected, Rule, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Kind {
    Word(String),
    Number(String),
    Text(String),
    Symbol(String),
    Newline,
    Indent,
    Dedent,
    End,
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub kind: Kind,
    pub span: Span,
}

impl Token {
    pub fn is(&self, text: &str) -> bool {
        matches!(&self.kind, Kind::Word(s) | Kind::Symbol(s) if s == text)
    }
}

pub(super) struct Lexer<'t> {
    text: &'t str,
    pos: usize,
    line_start: bool,
    brackets: usize,
    indents: Vec<usize>,
    pending: VecDeque<Token>,
    line_has_token: bool,
}

impl<'t> Lexer<'t> {
    pub fn new(text: &'t str) -> Self {
        Self {
            text,
            pos: 0,
            line_start: true,
            brackets: 0,
            indents: Vec::new(),
            pending: VecDeque::new(),
            line_has_token: false,
        }
    }

    fn rest(&self) -> &'t str {
        self.text.get(self.pos..).unwrap_or_default()
    }
    fn char(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn span(&self, start: usize) -> Span {
        Span { start, end: self.pos }
    }
    fn token(&self, start: usize, kind: Kind) -> Token {
        Token {
            kind,
            span: self.span(start),
        }
    }

    fn eof(&self, kind: Kind) -> Token {
        let last = self.text.strip_suffix('\n').unwrap_or(self.text);
        let end = last.strip_suffix('\r').unwrap_or(last).len();
        Token {
            kind,
            span: Span { start: end, end },
        }
    }

    pub fn next(&mut self) -> Result<Token, Rejected> {
        loop {
            if let Some(token) = self.pending.pop_front() {
                return Ok(token);
            }
            if self.line_start && self.brackets == 0 && self.char().is_some() {
                self.indentation()?;
                if !self.pending.is_empty() {
                    continue;
                }
            }
            let start = self.pos;
            let Some(c) = self.char() else {
                if self.line_has_token {
                    self.line_has_token = false;
                    return Ok(self.eof(Kind::Newline));
                }
                if self.indents.len() > 1 {
                    self.indents.pop();
                    return Ok(self.eof(Kind::Dedent));
                }
                return Ok(self.eof(Kind::End));
            };
            match c {
                ' ' => {
                    self.pos += 1;
                    continue;
                }
                '\n' | '\r' if c == '\n' || self.rest().starts_with("\r\n") => {
                    self.pos += if c == '\r' { 2 } else { 1 };
                    self.line_start = true;
                    if self.brackets == 0 && self.line_has_token {
                        self.line_has_token = false;
                        return Ok(self.token(start, Kind::Newline));
                    }
                    continue;
                }
                '#' => {
                    self.pos += 1;
                    while let Some(c) = self.char() {
                        if c == '\n' || self.rest().starts_with("\r\n") {
                            break;
                        }
                        if c.is_control() {
                            return Err(Rule::Character.reject(Span {
                                start: self.pos,
                                end: self.pos + c.len_utf8(),
                            }));
                        }
                        self.pos += c.len_utf8();
                    }
                    continue;
                }
                _ => {}
            }
            self.line_start = false;
            self.line_has_token = true;
            if c.is_ascii_alphabetic() || c == '_' {
                self.pos += 1;
                while self.char().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    self.pos += 1;
                }
                if matches!(self.char(), Some('\'' | '"'))
                    && !super::parse::keyword(self.text.get(start..self.pos).unwrap_or_default())
                {
                    return Err(Rule::Literal.reject(self.span(start)));
                }
                return Ok(self.token(
                    start,
                    Kind::Word(self.text.get(start..self.pos).unwrap_or_default().to_owned()),
                ));
            }
            if c.is_ascii_digit() {
                return self.number(start);
            }
            if c == '.' && self.rest().chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
                while self
                    .char()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                {
                    self.pos += 1;
                }
                return Err(Rule::Literal.reject(self.span(start)));
            }
            if c == '\'' || c == '"' {
                return self.string(start, c);
            }
            if !c.is_ascii() || c.is_control() || !"+-*/=!<>()[],:.;%@&|^~{}".contains(c) {
                self.pos += c.len_utf8();
                return Err(Rule::Character.reject(self.span(start)));
            }
            const OPS: [&str; 16] = [
                "//=", "**=", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "**", "//", "<<", ">>", ":=",
            ];
            let symbol = OPS
                .iter()
                .find(|op| self.rest().starts_with(**op))
                .copied()
                .map(str::to_owned)
                .unwrap_or_else(|| c.to_string());
            self.pos += symbol.len();
            if symbol == "(" || symbol == "[" {
                self.brackets += 1;
            }
            if symbol == ")" || symbol == "]" {
                self.brackets = self.brackets.saturating_sub(1);
            }
            return Ok(self.token(start, Kind::Symbol(symbol)));
        }
    }

    fn indentation(&mut self) -> Result<(), Rejected> {
        let start = self.pos;
        while self.char() == Some(' ') {
            self.pos += 1;
        }
        if matches!(self.char(), Some('#' | '\n')) || self.rest().starts_with("\r\n") || self.char().is_none() {
            self.line_start = false;
            return Ok(());
        }
        if self.char() == Some('\t') {
            return Err(Rule::Character.reject(Span {
                start: self.pos,
                end: self.pos + 1,
            }));
        }
        let indent = self.pos - start;
        let Some(&open) = self.indents.last() else {
            self.indents.push(indent);
            self.line_start = false;
            return Ok(());
        };
        if indent > open {
            self.indents.push(indent);
            self.pending.push_back(self.token(start, Kind::Indent));
        } else if indent < open {
            while self.indents.last().is_some_and(|&n| n > indent) {
                self.indents.pop();
                self.pending.push_back(self.token(start, Kind::Dedent));
            }
            if self.indents.last() != Some(&indent) {
                return Err(Rule::Indentation.reject(self.span(start)));
            }
        }
        self.line_start = false;
        Ok(())
    }

    fn number(&mut self, start: usize) -> Result<Token, Rejected> {
        while self.char().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let integer_end = self.pos;
        let mut valid = integer_end - start == 1 || !self.text.get(start..).unwrap_or_default().starts_with('0');
        if self.char() == Some('.') {
            self.pos += 1;
            let fraction = self.pos;
            while self.char().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            valid &= self.pos > fraction;
        }
        if self
            .char()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            valid = false;
            while self
                .char()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                self.pos += 1;
            }
        }
        if !valid {
            return Err(Rule::Literal.reject(self.span(start)));
        }
        Ok(self.token(
            start,
            Kind::Number(self.text.get(start..self.pos).unwrap_or_default().to_owned()),
        ))
    }

    fn string(&mut self, start: usize, delimiter: char) -> Result<Token, Rejected> {
        self.pos += 1;
        if self.rest().starts_with(&delimiter.to_string().repeat(2)) {
            self.pos += 2;
            return Err(Rule::Literal.reject(self.span(start)));
        }
        let mut value = String::new();
        loop {
            let Some(c) = self.char() else {
                return Err(Rule::Literal.reject(self.span(start)));
            };
            if c == '\n' || self.rest().starts_with("\r\n") {
                return Err(Rule::Literal.reject(self.span(start)));
            }
            let at = self.pos;
            self.pos += c.len_utf8();
            if c == delimiter {
                return Ok(self.token(start, Kind::Text(value)));
            }
            if c.is_control() {
                return Err(Rule::Character.reject(Span {
                    start: at,
                    end: self.pos,
                }));
            }
            if c != '\\' {
                value.push(c);
                continue;
            }
            let Some(escape) = self.char() else {
                return Err(Rule::Literal.reject(self.span(start)));
            };
            self.pos += escape.len_utf8();
            match escape {
                '\\' | '\'' | '"' => value.push(escape),
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'u' => {
                    let mut code = 0;
                    for _ in 0..4 {
                        let Some(digit) = self.char().and_then(|c| c.to_digit(16)) else {
                            return Err(Rule::Literal.reject(self.span(start)));
                        };
                        self.pos += 1;
                        code = code * 16 + digit;
                    }
                    let Some(decoded) = char::from_u32(code) else {
                        return Err(Rule::Literal.reject(self.span(start)));
                    };
                    value.push(decoded);
                }
                _ => return Err(Rule::Literal.reject(self.span(start))),
            }
        }
    }
}
