---
status: done
phase: 6
---

# 002 — Relational transactions on sietch's Snapshot Isolation

Wire ticket 001's row/catalog layer onto `sietch::TransactionalStore`,
and prove the isolation guarantee holds at the *relational* level (rows,
statements), not just assume it's inherited unchanged from the
underlying byte-key level.

## Scope
- A relational transaction API (begin/read row/write row/commit/abort)
  built on `sietch::Transaction`, translating row-level reads/writes
  through ticket 001's encoding.
- Multi-row, multi-statement transactions: a transaction touching several
  rows (possibly across tables) commits or aborts as one unit.
- Conflict handling: `sietch::Transaction`'s first-committer-wins conflict
  detection surfaced as a typed, distinguishable relational conflict
  error (an aborted transaction must be retriable, exactly like sietch's
  own ticket 007 proved at the byte-key level).
- Property test: for arbitrary concurrent (interleaved, single-threaded-
  simulated or real-threaded — whichever sietch's own transaction tests
  used) transactions over overlapping and disjoint row sets, the
  observed outcomes are consistent with *some* serial ordering of the
  committed transactions (a real serializability/snapshot-isolation
  check, not merely "it didn't crash").
- Differential test: run the same transaction workload against both the
  real row/txn layer and a plain in-memory reference model (a `HashMap`
  behind a mutex, or similar) applying the same serial ordering; final
  state must match.

## Done
- [x] Relational transaction API (`txn::Database::begin` -> `RelTransaction`: read/write/delete/commit/abort) over `sietch::Transaction`
- [x] Multi-row, multi-table transactions commit or abort as one unit
- [x] Conflicts surfaced as `DbError::Conflict { table, pk }`
- [x] Property test: concurrent commits match a serial replay in commit order (blind writes; see ADR-002 for scope)
- [x] Differential test against a `HashMap` reference model
- [x] Mutation-checked both tests
- [x] ADR-002 written (shared TransactionalStore, catalog index instead of scan)
