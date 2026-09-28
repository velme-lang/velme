//! Velme syntax: source files, lexer and layout pass, tokens and keywords (`language/10`).
#![forbid(unsafe_code)]

mod keyword;
mod lexer;
mod source;
mod token;

pub use keyword::Keyword;
pub use lexer::lex;
pub use source::SourceFile;
pub use token::{Punct, Token, TokenKind};

/// The language version this compiler implements and assumes for a file without a header (R-SYN-15, INV-8).
pub const LANGUAGE_VERSION: &str = "0.1";
