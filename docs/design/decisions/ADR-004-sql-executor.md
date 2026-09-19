# ADR-004: A SQL executor with static type checking, poisoned transactions, and two access paths

## Status
Accepted

## Context

Ticket 004 turns parsed statements into results: CREATE TABLE, INSERT,
SELECT (WHERE, ORDER BY, LIMIT), UPDATE, DELETE, and BEGIN/COMMIT/
ROLLBACK, on top of `txn`'s relational transactions.

## Findings while building it (reported plainly)

1. **sietch had to change after all.** ADR-002 avoided touching sietch by
   giving the catalog an explicit table-name index. That does not scale to
   row data: a SELECT needs every row of a table, and an index key holding
   every primary key would be rewritten on each INSERT. The right fix is
   the capability ADR-002 declined to add, so sietch ticket 014 added
   `Transaction::scan(prefix)` (snapshot view merged with the
   transaction's own buffered writes). It is additive; no existing sietch
   behaviour changed, and sietch's full suite still passes (123 tests).
   ADR-002's reasoning was right for the catalog and wrong as a general
   rule; this supersedes it for scans.
2. **A latent panic in `txn`.** `RelTransaction::write` indexed
   `values[primary_key]` directly, so a too-short row panicked instead of
   returning an error. Found by reading it while writing the executor;
   fixed and pinned by a test. The ticket-002 tests never wrote a short
   row.
3. **The differential test caught a real semantic bug.** `SELECT * FROM t
   WHERE n` (a non-boolean WHERE) failed on a table with rows but silently
   succeeded on an empty one, because type errors were only raised while
   evaluating a row. Whether a statement is an error must not depend on
   what data exists. Fix: a static type checker (`check_type`) run before
   any row is touched, covering WHERE clauses and UPDATE assignments.
   Only data-dependent errors (integer overflow, division by zero) remain
   runtime errors. Stated limitation: a NULL assigned to a NOT NULL column
   is still only caught when a row actually matches.

## Decision

**Autocommit plus explicit transactions.** A statement outside BEGIN runs
in its own transaction (so a multi-row INSERT that fails halfway leaves
nothing behind: verified by test and by the model). Inside BEGIN..COMMIT
statements share one `RelTransaction`.

**A failed statement poisons an explicit transaction** (PostgreSQL-style):
later statements return `TransactionAborted` until ROLLBACK, and COMMIT of
a poisoned transaction rolls back and reports the abort. Reason:
`sietch::Transaction` has no savepoints, so a half-applied statement
cannot be undone in place. Poisoning is the honest alternative to silently
committing partial statements.

**CREATE TABLE is refused inside a transaction.** DDL commits through the
catalog's own transaction; mixing it into a user transaction would need
catalog changes to be rolled back with it. Deferred deliberately.

**UPDATE cannot change the primary key.** It would be a delete plus an
insert with cross-row uniqueness checks (think `SET id = id + 1`). Out of
scope; reported as a typed error.

**Two access paths.** `access_path` returns a point lookup when the WHERE
clause is, or is an AND chain containing, `pk = <constant>` (either
operand order, constant of the pk's own type) and a full scan otherwise.
The whole WHERE clause is still applied to whatever rows the path
returns, so the optimization can only skip work, never change results;
the differential test biases predicates toward `id = k` to exercise this.

**SQL three-valued logic.** Comparisons and arithmetic with NULL yield
NULL; `FALSE AND NULL` is FALSE, `TRUE OR NULL` is TRUE; WHERE keeps only
TRUE. NULLs sort first ascending. There is no implicit coercion: `1 =
'a'` is a type error. AND/OR evaluate both sides (no short-circuit) so
errors are deterministic.

## Research question: what does append-only storage change here?

- UPDATE and DELETE never mutate: each is a new version or tombstone in
  sietch's log, and an open transaction's snapshot keeps reading the old
  versions (tested: a session mid-transaction does not see later commits).
- Conflicts are detected at row granularity on the keys a transaction
  wrote, surfaced as `DbError::Conflict { table, pk }` (tested with two
  sessions).
- Costs (scan time with many dead versions, interaction with compaction
  while long snapshots are open) are not yet measured; ticket 005's
  benchmark is where numbers belong.

## Testing

- 23 engine unit tests and 22 evaluator/executor semantics tests
  (three-valued logic, overflow, division by zero, type mismatches,
  constraints, ORDER BY/LIMIT/NULL ordering, poisoning, session conflicts,
  snapshot isolation, reopen, scripts, access-path selection, static type
  errors on an empty table).
- Differential test (200 cases of up to 40 statements): an independent
  `BTreeMap` reference with its own predicate evaluator, covering
  BEGIN/COMMIT/ROLLBACK and deliberately failing statements. Success or
  failure, rows returned, rows affected and final contents must all agree.
- Mutation-checked: treating a NULL WHERE result as TRUE, and skipping the
  WHERE re-check after an access path, are both caught.

## Alternatives Considered

1. **Per-row runtime type errors only.** Rejected after finding 3.
2. **Auto-rollback the whole transaction on any error.** Rejected: it
   hides which statement failed; poisoning matches established practice.
3. **Row-count-based planner or cost model.** Rejected: with primary-key
   lookup and full scan as the only paths, there is nothing to choose
   between.

## Consequences

- Known limitations: no joins, aggregates, GROUP BY, DISTINCT,
  secondary indexes, range scans on the primary key (the key encoding is
  order-preserving, so this is possible later), BYTES literals, or
  multi-column ORDER BY. A scan materializes the whole table in memory.
- Ticket 005 adds the `choamc` shell and a closing workload.
