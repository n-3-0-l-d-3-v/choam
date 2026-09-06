# CHOAM — THE DATABASE

> A relational database built on top of sietch, not SQLite.

## Why "CHOAM"

Combine Honnete Ober Advancer Mercantiles: the empire-spanning trade conglomerate that tracks and controls economic value across every House. Functionally it is the setting's central ledger — a system of record for structured, valuable data across many participants, exactly what a database is. It is also genuinely obscure (barely surfaces outside the books), so it reads as a real name rather than a description.

Part of **[ARRAKIS](https://github.com/n-3-0-l-d-3-v/arrakis)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 6 — QUEUED**

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
