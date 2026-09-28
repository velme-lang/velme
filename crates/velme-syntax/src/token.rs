//! Tokens produced by the lexer and layout pass (`language/10` §2.1, §3).

use std::fmt;

use serde::Serialize;
use velme_diagnostics::Span;

use crate::Keyword;

/// Punctuation (`language/10` §2.1), longest match first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Punct {
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `,`
    Comma,
    /// `:`
    Colon,
    /// `.`
    Dot,
    /// `=`
    Assign,
    /// `-`
    Minus,
    /// `+`
    Plus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `?`
    Question,
    /// `|`
    Pipe,
    /// `->`
    Arrow,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,
}

impl Punct {
    /// Two-character punctuation, checked before single characters.
    pub(crate) const DOUBLE: &[(&str, Punct)] = &[
        ("->", Punct::Arrow),
        ("==", Punct::EqEq),
        ("!=", Punct::NotEq),
        ("<=", Punct::LtEq),
        (">=", Punct::GtEq),
    ];

    /// The single-character punctuation spelled `c`, if any.
    pub(crate) fn single(c: char) -> Option<Punct> {
        Some(match c {
            '(' => Punct::LParen,
            ')' => Punct::RParen,
            '[' => Punct::LBracket,
            ']' => Punct::RBracket,
            '<' => Punct::Lt,
            '>' => Punct::Gt,
            ',' => Punct::Comma,
            ':' => Punct::Colon,
            '.' => Punct::Dot,
            '=' => Punct::Assign,
            '-' => Punct::Minus,
            '+' => Punct::Plus,
            '*' => Punct::Star,
            '/' => Punct::Slash,
            '?' => Punct::Question,
            '|' => Punct::Pipe,
            _ => return None,
        })
    }

    /// The punctuation as written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            Punct::LParen => "(",
            Punct::RParen => ")",
            Punct::LBracket => "[",
            Punct::RBracket => "]",
            Punct::Lt => "<",
            Punct::Gt => ">",
            Punct::Comma => ",",
            Punct::Colon => ":",
            Punct::Dot => ".",
            Punct::Assign => "=",
            Punct::Minus => "-",
            Punct::Plus => "+",
            Punct::Star => "*",
            Punct::Slash => "/",
            Punct::Question => "?",
            Punct::Pipe => "|",
            Punct::Arrow => "->",
            Punct::EqEq => "==",
            Punct::NotEq => "!=",
            Punct::LtEq => "<=",
            Punct::GtEq => ">=",
        }
    }
}

/// A budget `UNIT` (`language/10` §2.1), written directly after a `NUMBER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// `ms`
    Ms,
    /// `s`
    S,
    /// `kb`
    Kb,
    /// `mb`
    Mb,
}

impl Unit {
    /// The unit as written.
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Ms => "ms",
            Unit::S => "s",
            Unit::Kb => "kb",
            Unit::Mb => "mb",
        }
    }

    pub(crate) fn from_word(word: &str) -> Option<Unit> {
        [Unit::Ms, Unit::S, Unit::Kb, Unit::Mb]
            .into_iter()
            .find(|u| u.as_str() == word)
    }
}

/// What a token is (`language/10` §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `NAME`: an identifier that isn't a keyword.
    Name(String),
    /// A keyword or reserved word.
    Keyword(Keyword),
    /// `NUMBER`, as written (underscores kept); the parser checks and normalizes it (R-SYN-03).
    Number(String),
    /// `UNIT`: a unit written with no space after a `NUMBER`.
    Unit(Unit),
    /// `TEXT`, with escapes decoded.
    Text(String),
    /// `BLOCK_TEXT`: a `plan: |` block, normalized per D-21 and D-67.
    BlockText(String),
    /// Punctuation.
    Punct(Punct),
    /// End of a logical line.
    Newline,
    /// The start of a deeper-indented block.
    Indent,
    /// The end of an indented block.
    Dedent,
}

impl fmt::Display for TokenKind {
    /// How a learner sees the token in a message; never an internal token name (R-SYN-18).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Name(name) => write!(f, "`{name}`"),
            TokenKind::Keyword(kw) => write!(f, "`{kw}`"),
            TokenKind::Number(text) => write!(f, "`{text}`"),
            TokenKind::Unit(unit) => write!(f, "`{}`", unit.as_str()),
            TokenKind::Text(_) => f.write_str("some text"),
            TokenKind::BlockText(_) => f.write_str("a plan"),
            TokenKind::Punct(p) => write!(f, "`{}`", p.as_str()),
            TokenKind::Newline => f.write_str("the end of the line"),
            TokenKind::Indent => f.write_str("an indented line"),
            TokenKind::Dedent => f.write_str("the end of the indented block"),
        }
    }
}

/// A token and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Where it is. Layout tokens have an empty span at the position they stand for.
    pub span: Span,
}
