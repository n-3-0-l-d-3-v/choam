# Scope — choam

## CORE (required for this repo to be considered complete at all)
- A relational row/value encoding on top of `sietch::Store`'s byte
  keys/values (table + primary key -> sietch key; typed columns ->
  sietch value), with a schema (catalog) that itself lives in sietch so
  schema changes are crash-safe and versioned like data (ticket 001).
- Relational transactions wired onto `sietch::TransactionalStore`, with
  the isolation guarantee proven at the row/statement level, not just
  inherited by assumption from the byte-key level (ticket 002).
- A constrained SQL subset — `CREATE TABLE`, `INSERT`, `SELECT` (with
  `WHERE` over indexed and unindexed columns, at least one join type),
  `UPDATE`, `DELETE` — parsed and executed for real, differentially
  tested against an independent in-memory reference model (tickets
  003–00N, broken down once ticket 001/002 land and the real shape of
  the work is clearer).

## EXTENSION (required for full integration into the combined ecosystem)
- A real CLI/REPL (`choamc`) for interactive use and scripted workloads.
- A closing multi-table, multi-client workload with genuine concurrent
  transaction contention (mirroring sietch's own ticket 007 and
  muaddib/distrans's closing-ticket pattern), plus differential testing
  and honestly-measured benchmarks (e.g. point lookup vs. scan cost as
  the underlying log/B+Tree grows, transaction-conflict retry cost).

## EXPERIMENT (only attempted once CORE + EXTENSION are healthy)
- A secondary (non-primary-key) index type built on `sietch`'s existing
  B+Tree, to test whether relational secondary indexing changes anything
  about the append-only storage argument beyond what the primary-key
  encoding already established.
- A minimal query planner with more than one physical strategy per
  logical plan (e.g. choosing between a full scan and an index lookup),
  to see whether an append-only substrate changes what "the cheap plan"
  even means compared to a conventional in-place-update database.
