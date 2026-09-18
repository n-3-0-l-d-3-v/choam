---
status: open
phase: 6
---

# 001 — Row encoding and catalog over sietch

The foundation everything else in this repo builds on: a relational
row/value encoding on top of `sietch::Store`'s byte keys and values, and
a schema catalog that lives in sietch itself.

Per `docs/design/DATABASE.md`: `sietch::Store` already provides PUT/GET/
DELETE/SCAN/SNAPSHOT (verified against the actual code, not assumed) —
this ticket is not building those, it's building the relational encoding
that sits on top of them.

## Scope
- A table+primary-key encoding scheme mapping `(table_name, primary_key)`
  injectively into `sietch::Store`'s byte key space, with a reserved key
  prefix range so catalog entries and row data can never collide.
- Typed column values (at minimum: integers, text, bytes, booleans,
  nullable) with an encoding that round-trips exactly and preserves
  whatever ordering property `scan`/`scan_at` need for prefix scans to
  return rows in a sane order.
- A catalog: table definitions (name, columns, types, primary key)
  themselves stored as rows in sietch (via the reserved prefix), so
  `CREATE TABLE` is crash-safe and versioned exactly like data, with no
  separate non-sietch metadata file.
- Property test: for arbitrary table names, primary keys, and row
  values within the supported types, encode-then-decode round-trips
  exactly, and no two distinct (table, primary key) pairs ever produce
  the same sietch key.
- Property test: an arbitrary sequence of `CREATE TABLE` + row
  put/delete operations, replayed through a fresh `sietch::Store::open`
  (i.e. surviving a real close/reopen, exercising sietch's own crash-
  safety guarantees transitively), reads back identically.

Not started. Depends on `sietch::Store` (already complete, Phase 2).
