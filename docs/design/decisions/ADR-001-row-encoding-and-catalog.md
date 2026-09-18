# ADR-001: A fixed-width table-id key namespace, order-preserving keys with ordinary-encoded values, and a catalog that is just rows

## Status
Accepted

## Context

Ticket 001 needed to check a design assumption before doing anything
else: `docs/design/CONSTRAINTS.md` names PUT/GET/DELETE/SCAN/SNAPSHOT as
this phase's starting point, phrased as if they still needed building.
Reading `sietch::Store`'s actual source (not its README, which was
stale relative to the repo — this project's own stated rule is that the
repo is ground truth) showed all five already exist, plus real Snapshot
Isolation (`TransactionalStore`, sietch's own ticket 007). So this
ticket is not "build a KV layer" — it's "build the relational encoding
that sits on top of the KV layer that already exists," which is a
different, smaller, and differently-shaped piece of work than
`docs/design/DATABASE.md`'s first draft assumed before this check.

## Decision

**Two disjoint key namespaces, distinguished by their first byte.**
`0x00` is the catalog (table schemas, plus a table-id counter at the
bare `[0x00]` key); `0x01` is row data. A table name is validated
non-empty before it can ever become a catalog key, which is exactly
what guarantees it can never collide with the bare counter key — no
separate reserved-word list needed.

**Row keys use a fixed-width, catalog-assigned `table_id` (u32), never
the table's own name.** This was a deliberate rejection of the more
obvious "prefix with the table name" scheme: `sietch::Store::scan` is a
literal byte-prefix match, so table `"users"` would incorrectly also
match every row of a hypothetical table `"users2"` if row keys were
built from names directly. A fixed-width numeric id, compared byte-for-
byte, makes this impossible regardless of how table names relate to each
other as text — verified directly
(`different_tables_never_collide_regardless_of_key_type_or_value`).

**Primary keys are encoded order-preservingly; row *values* are not.**
Two distinct encodings of the same `Value` type exist on purpose:
`key::encode_key_component` (sign-bit-flipped big-endian integers, raw
bytes for text/bytes since a primary key is always the last — and, in
this ticket's scope, only — component of a row key, so no length prefix
is needed to avoid ambiguity) and `value::encode_value` (length-prefixed
variable-length fields, safe to concatenate and split back apart, used
for a row's stored value blob and for the catalog's own schema entries).
`sietch::Store::scan` only does prefix matching in this ticket's scope,
so the order-preserving property isn't exercised by anything yet — it's
built in now because it costs nothing extra and every future range
query (`WHERE id > 100`) this phase's SQL subset will want depends on
it existing from the start, not retrofitted later.

**No composite (multi-column) primary keys.** A stated, deliberate
scope limitation, not an oversight: `encode_key_component` is safe to
call with no length prefix specifically *because* it's always the last
and only component of the key. Composite keys would need either a
length-prefixed (and therefore non-order-preserving, defeating the
point above) encoding for every component but the last, or a more
complex escaping scheme. Single-column primary keys cover this ticket's
scope; a future ticket can revisit this if the SQL subset needs it.

**The catalog is not a separate metadata store — it's rows, in the same
`sietch::Store`, under the reserved namespace.** `CREATE TABLE` gets
exactly sietch's own crash-safety and append-only versioning for free.
This was the concrete, specific answer this ticket's ADR was supposed to
find to the phase's research question ("how does a relational engine's
design change when its storage can't mutate in place?") at this layer:
schema changes don't get a special code path at all — they're the same
kind of write as everything else, which is only possible because the
underlying store already treats every write as an immutable, versioned
append. A conventional in-place database usually *does* special-case
DDL (separate system catalogs, separate locking); here there was no
reason to.

## Testing

- `value.rs`, `key.rs`, `row.rs`, `codec.rs` (catalog's own schema-entry
  codec): unit tests for round-tripping every type, rejecting truncated/
  invalid input, and the specific edge cases each encoding's contract
  depends on (empty payloads, min/max integers, non-nullable primary
  keys).
- Property tests (`row`): arbitrary values and arbitrary full rows
  round-trip; distinct primary keys within one table (drawn from the
  *same* declared type — see below) never collide; distinct tables never
  collide regardless of key type or value on either side.
- Property test (`catalog`): an arbitrary sequence of `CREATE TABLE` +
  row put/delete operations, replayed through a real `Catalog`/`Store`
  close and reopen, reads back identically — every table, every live
  row, and nothing that was deleted.
- Mutation-checked: dropping `table_id` from `row_key_prefix` is caught
  immediately by the cross-table collision property; removing the
  duplicate-table-name check in `create_table` is caught by the existing
  unit test.

**A property-design mistake caught before it shipped, not after**: the
first draft of the same-table collision property drew two primary keys
of *independently* arbitrary types (rather than both from one
type chosen once) and asserted they must never collide. That's testing
a scenario no real table can ever be in — a table has exactly one
primary-key type — and an Integer's sign-flipped 8-byte encoding could
in principle coincidentally equal some Text value's raw UTF-8 bytes,
which would have failed the property for a "collision" that isn't a
real bug. Fixed before running it for real: `arb_column_type()` is
chosen once, then both primary keys are drawn `arb_value_of(that same
type)`, matching what a real table can actually contain.

## Alternatives Considered

1. **Row keys prefixed by table name.** Rejected — see above (the
   `"users"`/`"users2"` prefix-collision hazard).
2. **A separate on-disk file or `sietch` instance for the catalog.**
   Rejected: it would need its own crash-safety story instead of
   inheriting sietch's, duplicating work sietch's ticket 003 already did
   exhaustively (byte-offset crash injection).
3. **Composite primary keys from the start.** Deferred — see above.
4. **Type tags stored per-value instead of driven by the schema.**
   Rejected: the schema already knows every column's type (that's what
   a schema *is*), so a redundant per-value tag would only add bytes and
   a second thing that could disagree with the schema.

## Consequences

- Ticket 001 closes. Ticket 002 (relational transactions) can build
  directly on `row_key`/`encode_row`/`Catalog` — a transaction's
  reads/writes translate through this same encoding, wrapped in
  `sietch::Transaction` instead of raw `Store` calls.
- **Known limitation, stated honestly**: no composite primary keys (see
  above) and no secondary indexes yet (`SCOPE.md`'s EXPERIMENT item).
  A table's only lookup path today is by its single primary-key column,
  or a full table scan via `row_key_prefix`.
- No performance measurement yet (row encode/decode cost, catalog
  lookup cost) — deferred to this phase's eventual closing/benchmarking
  ticket, per this project's established pattern of measuring rather
  than assuming.
