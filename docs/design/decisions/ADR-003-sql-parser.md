# ADR-003: A hand-written SQL parser with a real AST, reserved keywords, and a fully parenthesized renderer

## Status
Accepted

## Context

Ticket 003 needs a parser for a small SQL subset (CREATE TABLE, INSERT,
SELECT with WHERE/ORDER BY/LIMIT, UPDATE, DELETE, BEGIN/COMMIT/ROLLBACK).
The interesting question for this repo is not the grammar, which is
textbook, but what the parse output should be and how to prove the parser
correct rather than merely exercised.

## Decision

**A real AST, unlike chakobsa.** chakobsa's premise is to avoid an AST and
build SSA directly. That premise does not transfer here: SQL is
declarative, the executor needs to inspect and rewrite the whole
statement (for example, to spot a primary-key equality in a WHERE clause
and choose a point lookup over a scan), and there is no incremental
data-flow construction to fuse into the parse. A plain tree is the right
tool. This is recorded so nobody reads the different choice as
inconsistency.

**Hand-written recursive descent with precedence climbing**, no parser
generator. Precedence, lowest to highest: OR, AND, NOT, comparison and
IS [NOT] NULL, additive, multiplicative, unary minus, primary.
Binary operators are left-associative.

**Keywords are reserved and case-insensitive.** A table or column named
`select` or `key` is a parse error. Cheaper and less ambiguous than
context-sensitive keywords, and it lets the renderer emit identifiers
unquoted.

**Negative numbers are `Unary(Neg, Integer)`, not negative literals.** The
lexer only produces non-negative integers, so the full i64 range minus
`i64::MIN` is expressible as a literal, and `-x` and `-5` are the same
construct. Consequence, stated plainly: `i64::MIN` cannot be written as a
literal (`-9223372036854775808` overflows when the lexer reads the
positive part). It is reachable as `-9223372036854775807 - 1`.

**`Display` fully parenthesizes every expression.** `(a + (b * c))`, never
relying on precedence. Rendering therefore cannot be ambiguous, which is
what makes the round-trip property below a sharp test of the parser's
precedence and associativity rather than a test of the renderer's
minimal-parenthesis logic.

**Errors are typed and positioned**: lexer errors carry byte offsets;
parse errors say what was expected and what was found at which offset, or
that input ended. Nothing panics on any input.

## Testing

- Lexer and parser unit tests: operator longest-match, quote escaping,
  integer bounds, keyword-prefixed identifiers, precedence and
  associativity, every clause, script splitting, error variants and
  positions, reserved words, empty lists, LIMIT operand.
- Property tests (512 cases each): for arbitrary generated statements
  (nested expressions to depth 4, every statement kind),
  `parse(render(ast)) == ast`; arbitrary Unicode text never panics; and
  random sequences of SQL-shaped tokens (unbalanced parentheses, stray
  quotes, half-statements) never panic.
- Mutation-checked: making `<=` parse as `<` fails the round-trip
  property immediately.

No parser bugs surfaced during development; the tests passed on the first
full run apart from a clippy doc-formatting lint. That is reported as-is:
this ticket is well-trodden ground, and the value of the tests here is
regression protection for the executor work built on top.

## Alternatives Considered

1. **A parser-combinator or generator crate.** Rejected: an extra
   dependency for a grammar this small, and worse error positions.
2. **Minimal-parenthesis rendering.** Rejected for the reason above.
3. **Negative literals in the lexer.** Rejected: makes `a-1` versus
   `a -1` ambiguous at token level.

## Consequences

- Ticket 004 (executor) can pattern-match on `Statement`/`Expr` directly.
- Known limitations: no joins, subqueries, aggregates, GROUP BY,
  DISTINCT, or multi-column ORDER BY; no quoted identifiers; no comments;
  no `i64::MIN` literal. All are deliberate scope cuts, not oversights.
