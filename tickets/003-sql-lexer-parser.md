---
status: open
phase: 6
---

# 003 — SQL subset: lexer and parser

## Scope
- `crates/sql`: hand-written lexer and recursive-descent/precedence-climbing parser for a deliberately small SQL subset.
- Statements: CREATE TABLE, INSERT (multi-row, optional column list), SELECT (projection, WHERE, ORDER BY one column, LIMIT), UPDATE, DELETE, BEGIN/COMMIT/ROLLBACK.
- Expressions: integer/string/boolean/NULL literals, columns, + - * /, comparisons, AND/OR/NOT, IS [NOT] NULL, parentheses.
- Typed errors carrying a byte position; never panic on arbitrary input.
- Property tests: arbitrary text never panics; render(AST) -> parse round-trips for arbitrary ASTs.
