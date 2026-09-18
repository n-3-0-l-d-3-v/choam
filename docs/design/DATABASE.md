# THE DATABASE — architecture (Phase 6)

## Overview

CHOAM builds a relational database that runs **entirely on `sietch`'s
own storage engine** — a real cross-repo dependency, the same way
chakobsa's codegen depends on mentat's `isa`/`vm`. Per
`docs/design/CONSTRAINTS.md`, no SQLite, no borrowed storage layer: every
byte this database ever writes goes through `sietch::Store`, which can
never overwrite a byte in place. The research question this phase exists
to answer: **how does a relational query engine's design change when its
storage layer cannot mutate in place?**

## What `sietch` already provides (verified against the actual code, not assumed)

`sietch::Store` already exposes exactly the primitives
`docs/design/CONSTRAINTS.md` names as this phase's starting point:
`put`/`delete`/`apply_batch` (a batch shares one `fsync`), `get`/`get_at`,
`scan`/`scan_at` (prefix scan), and `snapshot`/`hold_snapshot`. Multi-
version reads and snapshot isolation are not something CHOAM needs to
build from scratch: `sietch::TransactionalStore`/`Transaction` already
gives real Snapshot Isolation (first-committer-wins conflict detection),
proven under genuine multi-threaded contention in sietch's own ticket
007. **Checking this against the code before planning ticket 001** (the
same discipline muaddib's ADR-003→004 and distrans's ADR-003 both relied
on) changes this phase's shape: ticket 001 is not "build PUT/GET/SCAN/
SNAPSHOT" — that already exists — it is "build the relational encoding
and API surface *on top of* sietch's existing KV+MVCC primitives," and
ticket 002 is not "build transactions from scratch" — it is "wire
`TransactionalStore` in as the relational layer's transaction mechanism
and prove the relational-level guarantees it's supposed to provide
(isolation between concurrent statements, not just raw key writes)
actually hold once real rows, not raw byte blobs, are involved."

## Layer map (planned; tickets refine this as they land)

```text
crates/row       -- row/value encoding on top of sietch's byte keys and
                     values: a table+primary-key encoding scheme for
                     Store's key space, typed column values, row
                     (de)serialization (ticket 001, DONE)
crates/catalog   -- schema storage: table/column definitions themselves
                     live in sietch too (a reserved key range), so
                     schema changes get the same crash-safety and
                     snapshot isolation as data (ticket 001, DONE)
crates/txn       -- the relational transaction API wired onto
                     sietch::TransactionalStore; whatever isolation gap
                     exists between "byte-key snapshot isolation" and
                     "row/statement-level isolation" gets closed and
                     proven here (ticket 002)
crates/sql       -- a constrained SQL subset: lexer/parser -> a small
                     logical plan -> execution against crates/row+txn.
                     Built bottom-up (ticket 003+), scope narrowed
                     deliberately (see SCOPE.md) rather than chasing SQL
                     completeness
crates/cli       -- a real CLI/REPL (`choamc`), mirroring every other
                     phase's closing-visible-artifact pattern
crates/workload  -- closing ticket: a real multi-table workload,
                     concurrent-client transaction testing (real
                     contention, like sietch's own ticket 007), and
                     comparison against a reference in-memory relational
                     model
```

## Invariants each layer must prove (not just assert)

- **row/catalog**: a table+primary-key encoding is injective (two
  distinct (table, key) pairs never collide in sietch's byte-key space)
  and round-trips every supported column type exactly.
- **txn**: two concurrent transactions touching disjoint rows never
  conflict; two touching the same row are resolved by sietch's existing
  first-committer-wins rule, with the relational layer surfacing that as
  a typed conflict error, not silent data loss.
- **sql**: for every supported statement, the executed result matches an
  independent reference (a plain in-memory relational model), for
  arbitrary generated schemas/data within the supported subset.

## Research question

> How does a relational query engine's design change when its storage
> layer cannot mutate in place?

Each ticket's ADR should record concretely where sietch's append-only
discipline showed up as a real constraint on the relational design (e.g.
how a table scan interacts with `scan_at`'s versioning, what an UPDATE
or DELETE actually costs when the underlying store is append-only, how
compaction and long-running snapshot transactions interact) — mirroring
how mentat's ISA and sietch's own compaction protocol turned up real,
specific findings rather than generic ones.
