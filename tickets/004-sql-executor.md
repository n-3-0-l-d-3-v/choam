---
status: done
phase: 6
---

# 004 — SQL executor

Execute parsed statements over `txn`: CREATE TABLE, INSERT, SELECT (WHERE / ORDER BY / LIMIT), UPDATE, DELETE, BEGIN / COMMIT / ROLLBACK.

## Done
- [x] `crates/engine`: sessions (autocommit and explicit transactions), executor, point-lookup vs full-scan access paths
- [x] SQL three-valued logic; static type checking so type errors do not depend on data
- [x] Poisoned transactions (no savepoints in sietch)
- [x] sietch ticket 014: `Transaction::scan` (cross-repo, additive)
- [x] txn: `RelTransaction::scan`; fixed a latent panic on short rows
- [x] Differential test against an independent in-memory model (found a real bug, fixed); mutation-checked
- [x] ADR-004
