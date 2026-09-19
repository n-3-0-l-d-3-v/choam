# CHOAM — THE DATABASE

> A relational database built on top of sietch, not SQLite.

## Why "CHOAM"

Combine Honnete Ober Advancer Mercantiles: the empire-spanning trade conglomerate that tracks and controls economic value across every House. Functionally it is the setting's central ledger — a system of record for structured, valuable data across many participants, exactly what a database is. It is also genuinely obscure (barely surfaces outside the books), so it reads as a real name rather than a description.

Part of **[ARRAKIS](https://github.com/n-3-0-l-d-3-v/arrakis)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 6 — ACTIVE.** See [docs/design/DATABASE.md](docs/design/DATABASE.md)
for the layer map and what checking `sietch`'s actual code (rather than
assuming) showed: PUT/GET/DELETE/SCAN/SNAPSHOT and real Snapshot
Isolation already exist there (Phase 2, tickets 002 and 007).

**Ticket 001 (row encoding and catalog) is done.** `crates/row`: two
disjoint key namespaces (catalog vs. row data) distinguished by their
first byte; row keys use a fixed-width, catalog-assigned `table_id`
rather than the table's own name, specifically so one table's name can
never be a byte-prefix of another's row keys (`"users"` vs. `"users2"`).
Primary keys are encoded order-preserving (sign-flipped big-endian
integers, raw bytes for text/bytes); row values use an ordinary
length-prefixed codec — two distinct encodings of the same `Value` type,
on purpose. `crates/catalog`: table schemas are stored as ordinary rows
in the same `sietch::Store`, under the reserved namespace — `CREATE
TABLE` gets exactly sietch's own crash-safety and versioning for free,
with no separate metadata file, which is this ticket's concrete answer
to the phase's research question at this layer. A property-design
mistake was caught before it shipped: an early version of the
same-table key-collision property drew two primary keys of
*independently* arbitrary types, which could fail on a coincidental
encoding collision no real table (which has exactly one primary-key
type) could ever produce — fixed to draw both from one chosen type.
32 unit tests, 5 property tests, all mutation-checked. See
[ADR-001](docs/design/decisions/ADR-001-row-encoding-and-catalog.md).

**Ticket 002 (relational transactions) is done.** `crates/txn`:
`Database::begin` starts a `RelTransaction` (read/write/delete/commit/
abort by table name and primary key), a thin translation over one
`sietch::Transaction`, so multi-row, multi-table commits are atomic and
Snapshot Isolation with first-committer-wins is inherited unchanged.
Conflicts surface as `DbError::Conflict { table, pk }`. Checking
assumptions first found that `TransactionalStore` has no `scan` and that
the catalog and row data must share one store handle, so the catalog
moved onto `TransactionalStore` with an explicit, transactionally
maintained table-name index (a disclosed behavior change: concurrent
`CREATE TABLE` can now conflict and must be retried). Tests: a
concurrent-commit property test checked against a serial replay (blind
writes; SI's write-skew gap is stated, not hidden) and a differential
test against a `HashMap`, both mutation-checked. See
[ADR-002](docs/design/decisions/ADR-002-relational-transactions.md).

**Ticket 003 (SQL lexer and parser) is done.** `crates/sql`: a hand-written
lexer and precedence-climbing parser for a small SQL subset, producing a
real AST (the opposite choice from chakobsa, for stated reasons). The
renderer fully parenthesizes, so a property test proves
`parse(render(ast)) == ast` for arbitrary statements; the parser never
panics on arbitrary or SQL-shaped garbage. See
[ADR-003](docs/design/decisions/ADR-003-sql-parser.md).

**Ticket 004 (SQL executor) is done.** `crates/engine`: autocommit and
explicit BEGIN/COMMIT/ROLLBACK sessions, SQL three-valued logic, point
lookup vs full scan, and a static type checker. Building it needed a
capability sietch lacked (`Transaction::scan`, added as sietch ticket 014)
and exposed a latent panic in `txn`. The differential test against an
independent reference model caught a real bug (type errors that depended
on whether rows existed), now fixed. See
[ADR-004](docs/design/decisions/ADR-004-sql-executor.md).

See [tickets/](tickets/) for the live phase-by-phase ticket board and
[docs/design/](docs/design/) for constraints, invariants and architecture
decision records.

## The constraint

The database must run entirely on the project's own immutable storage engine, starting from PUT/GET/DELETE/SCAN/SNAPSHOT before any relational layer exists.

## What the constraint forces

Transactions and MVCC on top of an append-only substrate, real concurrent-client testing, and a constrained SQL subset built bottom-up.

## Research question

> How does a relational query engine's design change when its storage layer cannot mutate in place?

## Sibling repositories

- [mentat](https://github.com/n-3-0-l-d-3-v/mentat) — THE MACHINE (COMPLETE)
- [chakobsa](https://github.com/n-3-0-l-d-3-v/chakobsa) — THE LANGUAGE (QUEUED)
- [muaddib](https://github.com/n-3-0-l-d-3-v/muaddib) — THE KERNEL (QUEUED)
- [sietch](https://github.com/n-3-0-l-d-3-v/sietch) — THE VAULT (ACTIVE)
- [distrans](https://github.com/n-3-0-l-d-3-v/distrans) — THE WIRE (QUEUED)
- [landsraad](https://github.com/n-3-0-l-d-3-v/landsraad) — THE COLONY (QUEUED)
- [ghola](https://github.com/n-3-0-l-d-3-v/ghola) — THE HISTORY (QUEUED)
- [shai-hulud](https://github.com/n-3-0-l-d-3-v/shai-hulud) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
