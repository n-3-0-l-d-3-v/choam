# ADR-002: A shared TransactionalStore, an explicit catalog index instead of scan, and a relational transaction API that's a thin translation over sietch::Transaction

## Status
Accepted

## Context

Ticket 002's stated scope was a relational transaction API (begin/read
row/write row/commit/abort) on top of `sietch::Transaction`, with
sietch's first-committer-wins conflicts surfaced as a typed, attributable
relational error, plus a property test proving concurrent outcomes are
consistent with some serial ordering and a differential test against a
plain reference model.

Before writing any of that, this ticket re-checked an assumption ticket
001 had made implicitly: that the catalog (`Catalog`, wrapping a plain
`sietch::Store`) and the new row-data transaction layer could each keep
their own storage handle. They can't. `sietch::Store` is documented as
single-writer and unsynchronized — two `Store` (or `TransactionalStore`)
instances opened on the same directory at once is unsafe, full stop, not
merely undesirable. Since the ticket's own scope requires row-data
transactions to run against the same directory the catalog already
manages, the catalog and the new relational layer must share exactly one
`TransactionalStore` handle.

That forced `Catalog` off plain `Store` and onto `TransactionalStore` —
which surfaced a second, unadvertised gap: `TransactionalStore`/
`Transaction` expose only `get`/`put`/`delete`/`begin`/`commit`/`abort`/
`compact`. There is no `scan`, only on the raw, non-transactional `Store`
that `TransactionalStore` wraps and does not expose. Ticket 001's
`Catalog::list_tables` depended on `Store::scan` over the catalog
namespace. Moving the catalog onto `TransactionalStore` broke that
method outright — not a bug being fixed, a real capability gap between
what ticket 001 assumed the storage layer offered going forward and what
`TransactionalStore` actually offers.

## Decision

**The catalog and the row-data transaction layer share one
`TransactionalStore`.** `Catalog::open` now calls
`TransactionalStore::open`, and `Catalog::store()` returns a reference
to it; `txn::Database` is built directly on top of `Catalog`, calling
`self.catalog.store().begin()` for every relational transaction. There
is exactly one storage handle for the whole crate's data, catalog
entries and row data alike, matching the actual safety contract
`sietch::Store` documents rather than working around it.

**An explicit, transactionally-maintained table-name index, instead of
adding `scan` to sietch.** Two ways to close the `scan` gap were on the
table: add a `scan`/`scan_at` method to `sietch::TransactionalStore`
itself, or keep the gap on the `sietch` side and redesign the catalog's
own key layout so it never needs a scan. This ADR chooses the second.
Sietch's storage phase is already marked `COMPLETE` in this project's
phase table — reopening a finished, already-audited phase's repo to add
a capability one downstream consumer wants is a bigger, riskier change
than adding one more catalog key inside choam, which is still active
work. The catalog namespace (`row::CATALOG_NAMESPACE`, `0x00`) gained a
second tag byte to make room:

```text
[0x00, 0x00]                   -- next-table-id counter (was [0x00])
[0x00, 0x01]                   -- table-name index (new)
[0x00, 0x02] ++ table name     -- one entry per table's schema (was [0x00] ++ name)
[0x01] ++ table_id (4 BE) ++ pk -- row data (unchanged)
```

`Catalog::create_table` writes the table entry, the incremented counter,
and the updated index **in one `sietch::Transaction`**, so the index can
never drift from the entries it lists — a torn write (entry written,
index not updated, or vice versa) is exactly what sietch's own
first-committer-wins conflict detection and atomic `apply_batch` already
rule out; this ADR just makes create_table use that machinery instead of
issuing three independent `Store::put` calls the way ticket 001's
version did. A side effect worth naming honestly: `create_table` is now
a serialization point — two concurrent `CREATE TABLE` calls that would
have succeeded independently under ticket 001's raw-`Store` version can
now race on the shared counter/index keys, and the loser gets
`CatalogError::ConcurrentCreateTable` and must retry. This is a real
behavior change, not just an implementation swap, and is covered by
`concurrent_create_table_calls_never_corrupt_the_counter_or_index`
(8 threads, every one racing to create a distinct table, retrying on
conflict, asserting the final counter/index end up exactly right).

**`row::split_row_key`/`decode_key_component` translate a raw conflict
key back into a table and primary key.** `sietch::TxnError::Conflict`
carries only the raw byte key that lost the race — meaningless outside
sietch. `txn::DbError::Conflict { table, pk }` is built by splitting the
key into `(table_id, pk_bytes)`, matching `table_id` against
`Catalog::list_tables()`, and decoding `pk_bytes` as that table's
declared primary-key type. If any step fails — not reachable in this
ticket's scope, since there's no `DROP TABLE` to make a `table_id` go
stale — `DbError::UnrecognizedConflict(key)` is the fallback rather than
a panic.

**`txn::RelTransaction` adds no isolation semantics of its own.** It's a
thin, schema-aware wrapper over exactly one `sietch::Transaction`:
`read`/`write`/`delete` translate a table name + primary key into
`row::row_key` plus `encode_row`/`decode_row`, and `commit`/`abort`
delegate straight through. It inherits Snapshot Isolation and
first-committer-wins write-write conflict detection completely
unchanged — this ticket's job was making that usable relationally and
attributable, not building a different isolation level.

## Testing

- `catalog`: unit tests updated for the new key layout and
  `TransactionalStore`-backed `Catalog`; a new
  `concurrent_create_table_calls_never_corrupt_the_counter_or_index`
  test (8 threads racing `CREATE TABLE`, retrying on
  `ConcurrentCreateTable`, asserting exactly `n` unique ids and exactly
  `n` listed tables at the end).
- `catalog`'s property test was refocused to catalog-only concerns
  (create/reopen/list) — it can no longer reach into a raw `Store` to
  put/delete row data now that `Catalog` doesn't expose one.
- `row`: `key.rs` unit tests extended for the sub-tagged catalog
  namespace, plus new tests for `split_row_key` (recovers `table_id`
  and rejects catalog keys) and `decode_key_component` (round-trips
  every `ColumnType`, inverse of `encode_key_component`).
- `txn`: 10 unit tests covering same-transaction read-your-writes,
  isolation from other transactions before commit, atomic multi-table
  commit, abort, delete, unknown-table rejection, conflict attribution
  (table + primary key named correctly), whole-transaction rollback on a
  partial conflict, snapshot pinning against a later commit, and
  surviving a real close/reopen.
- `txn`'s **serializability property test**
  (`concurrent_commits_match_a_serial_replay_in_commit_order`): multiple
  threads commit blind writes (no read informs a write) to a small
  shared keyspace, retrying on `DbError::Conflict`; every successful
  commit is appended to a log under the same lock a real committer would
  need anyway, so log order and commit order can't race apart; the
  database's final state must equal a plain `HashMap` replayed in that
  logged order. **Scope, stated honestly**: this proves outcomes are
  serializable for blind-write workloads, not in general — Snapshot
  Isolation's known gap is write skew (a transaction reads a key it
  doesn't write and writes based on that read, racing another
  transaction doing the mirror image), which needs cross-key read-write
  dependencies this test doesn't construct and this ticket's relational
  API doesn't yet need to defend against (no cross-row invariants are
  enforced anywhere in this phase yet). Mutation-checked: making
  `commit()` swallow `TxnError::Conflict` as a false success is caught
  immediately (a logged "commit" that never actually applied leaves the
  real database's state short of the model's).
- `txn`'s **differential test**
  (`matches_a_plain_hashmap_reference_model`): an arbitrary sequence of
  single-statement put/get/delete transactions, each committed
  immediately, checked against a `HashMap` after every operation and
  again on the whole table at the end. Mutation-checked: disabling
  `delete` is caught immediately.

## Alternatives Considered

1. **Add `scan`/`scan_at` to `sietch::TransactionalStore`.** Rejected —
   see above; sietch's storage phase is complete, and the explicit-index
   approach needed no changes to an already-shipped, already-audited
   repo.
2. **Keep the catalog on plain `Store`, give row data its own
   `TransactionalStore` on a separate subdirectory.** Rejected: two
   separate `sietch` instances means two separate crash-safety domains —
   a `CREATE TABLE` and the first `INSERT` into it could end up on
   opposite sides of a crash with no way to make them consistent with
   each other, which defeats the actual point of transactions here.
3. **Give `RelTransaction` its own conflict-retry loop internally
   (auto-retry on `DbError::Conflict` up to N times).** Rejected: retry
   policy (how many times, backoff, whether to retry at all) is a
   caller-level decision — this ticket's differential and property tests
   both retry explicitly at the call site, which is also what any real
   client would need to do, so hiding it inside `commit()` would remove
   information (a client can't tell "committed" from "committed after
   3 silent retries with side effects in between") for no benefit.

## Consequences

- Ticket 002 closes. The relational transaction API
  (`txn::Database`/`txn::RelTransaction`) is the foundation the SQL
  subset (whatever ticket comes next in this phase) will sit on.
- **Behavior change from ticket 001, stated honestly**: `CREATE TABLE`
  now goes through a transaction touching a shared counter and index
  key, so concurrent `CREATE TABLE` calls can conflict and must be
  retried by the caller. Ticket 001's version never had this — this is
  the direct, disclosed cost of closing the `scan` gap with an explicit
  index instead of leaving the catalog's table list denormalized across
  a scan.
- **Known limitation, stated honestly**: `RelTransaction` inherits
  Snapshot Isolation's write-skew gap unchanged (see Testing, above). If
  a future ticket needs a cross-row invariant enforced under
  concurrency, this will need addressing then — not a silent gap, an
  explicitly deferred one.
- No secondary indexes or composite primary keys yet — unchanged
  limitations carried over from ADR-001, not touched by this ticket.
