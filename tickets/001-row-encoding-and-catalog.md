---
status: done
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
- [x] A table+primary-key encoding scheme mapping `(table_name, primary_key)`
      injectively into `sietch::Store`'s byte key space, with a reserved
      key prefix range so catalog entries and row data can never collide
      — `crates/row`'s `CATALOG_NAMESPACE`/`ROW_NAMESPACE`, a fixed-width
      catalog-assigned `table_id` (not the table's own name) prefixing
      every row key.
- [x] Typed column values (integers, text, bytes, booleans, nullable)
      with an encoding that round-trips exactly and preserves order for
      primary-key components specifically (`key::encode_key_component`),
      distinct from the ordinary value codec used for a row's stored
      blob (`value::encode_value`).
- [x] A catalog: table definitions stored as rows in sietch (via the
      reserved prefix), so `CREATE TABLE` is crash-safe and versioned
      exactly like data, with no separate non-sietch metadata file —
      `crates/catalog`.
- [x] Property test: arbitrary table names/primary keys/row values
      round-trip exactly; no two distinct (table, primary key) pairs
      ever produce the same sietch key (checked both within one table,
      drawing both keys from the same declared type, and across
      different tables regardless of type).
- [x] Property test: an arbitrary sequence of `CREATE TABLE` + row
      put/delete operations, replayed through a real `sietch::Store`
      close and reopen, reads back identically.

32 unit tests, 5 property tests. See
`docs/design/decisions/ADR-001-row-encoding-and-catalog.md`.
