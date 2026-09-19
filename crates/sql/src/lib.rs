//! A constrained SQL subset: lexer, parser and syntax tree (ticket 003).
//! See `docs/design/decisions/ADR-003-sql-parser.md`.

mod ast;
mod lexer;
mod parser;

pub use ast::{BinaryOp, ColumnSpec, Expr, Literal, OrderBy, Projection, Statement, UnaryOp};
pub use lexer::{lex, LexError, Token};
pub use parser::{is_keyword, parse, parse_script, ParseError};
