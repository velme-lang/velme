//! Velme syntax: source files, lexer and layout pass, tokens, keywords, parser and AST (`language/10`).
#![forbid(unsafe_code)]

pub mod ast;
mod builtin_type;
mod keyword;
mod lexer;
mod parser;
mod source;
mod token;

pub use builtin_type::BuiltinType;
pub use keyword::Keyword;
pub use lexer::lex;
pub use parser::parse;
pub use source::SourceFile;
pub use token::{Punct, Token, TokenKind, Unit};

/// The language version this compiler implements and assumes for a file without a header (R-SYN-15, INV-8).
pub const LANGUAGE_VERSION: &str = "0.1";
