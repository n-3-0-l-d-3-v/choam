---
status: open
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

Not started. Depends on ticket 001 and `sietch::TransactionalStore`
(already complete, Phase 2, ticket 007).
