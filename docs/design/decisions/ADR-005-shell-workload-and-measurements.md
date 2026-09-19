# ADR-005: The choamc shell, a contention workload, and first measurements

## Status
Accepted (closes Phase 6)

## Context
Ticket 005 makes the database usable (`choamc`), proves it under real
concurrent load, and measures it, following the pattern of every earlier
phase's closing ticket.

## Decision
- **`choamc`** reads SQL from `-c`, `-f` or stdin (prompting only on a
  terminal), prints aligned tables and row counts, supports `.tables`,
  `.schema`, `.help`, `.quit`, sends errors to stderr, keeps going after an
  error, and exits 1 if any failed (2 for usage/IO errors). Statement
  splitting respects `;` inside string literals. Tested by running the real
  binary against a real directory: piped session, persistence across
  separate processes, error handling, transactions and dot commands.
- **Bank workload** (`crates/engine/tests/bank.rs`): 6 threads x 25
  transfers over 4 accounts, each a read-modify-write inside BEGIN..COMMIT
  with retry on `Conflict`. The final balance of every account must equal
  its initial value plus the net of committed transfers, and total money
  must be conserved. A lost update would break this. In the recorded run
  290 conflicts were detected and retried, so the contention is real, and
  the invariant held. Mutation-checked: making `commit` swallow conflicts
  fails it (account 0 ended at 970, expected 990).
- Concurrency here is the honest kind: real threads on one
  `TransactionalStore` behind a mutex, as in sietch ticket 007.

## Measurements (release build, one machine, single run, not tuned)
| Operation | Result |
|---|---|
| autocommit INSERT (one fsync each) | ~890 us/row |
| INSERT 5,000 rows in one transaction | ~8.6 us/row |
| point lookup `WHERE id = k` | ~2.9 us |
| full scan with non-key filter, 5,200 rows | ~1.9 ms |
| UPDATE every row (5,200) | ~37 ms |
| same scan after every row has 2 versions | ~1.9 ms |

Reading them honestly:
- Batching writes into a transaction is ~100x cheaper per row than
  autocommit, because the cost is the durable append (fsync), which a
  transaction pays once (sietch's `apply_batch`).
- The point lookup is ~650x faster than a scan of the same table, so the
  access-path choice from ADR-004 matters.
- Answering the phase's research question with data: at this size,
  append-only versioning costs the scan nothing measurable (1.88 ms vs
  1.92 ms with two versions per row), because sietch keeps its version
  index in memory and `scan_at` picks the visible version without touching
  dead ones. What was **not** measured: much larger tables, many versions
  per key, or a long-open snapshot preventing compaction. Those remain
  open, not shown to be fine.
- These are single runs on one machine; treat them as orders of magnitude.

## Consequences
- Phase 6 is complete. Known limits are unchanged from ADR-004 (no joins,
  aggregates, secondary indexes, or PK range scans; scans materialize the
  table).
- The `arrakis` repo's earlier history contains redundant mirror commits
  for choam ticket 003, caused by re-running the mirror script with a stale
  base commit. Content is correct (tree hashes match); history was not
  rewritten. The script now records its last mirrored commit.
