//! Relational row/value encoding on top of `sietch::Store`'s flat byte
//! keys and values (ticket 001). See `docs/design/DATABASE.md` and
//! `docs/design/decisions/ADR-001-row-encoding-and-catalog.md`.

mod key;
mod row;
mod value;

pub use key::{
    catalog_counter_key, catalog_table_key, row_key, row_key_prefix, CATALOG_NAMESPACE,
    ROW_NAMESPACE,
};
pub use row::{decode_row, encode_row, ColumnDef, RowError, Schema, SchemaError};
pub use value::{decode_value, encode_value, ColumnType, Value, ValueError};
