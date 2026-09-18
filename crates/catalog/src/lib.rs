//! The catalog layer (ticket 001): table schemas stored as rows in the
//! same `sietch::Store` as data. See `docs/design/DATABASE.md` and
//! `docs/design/decisions/ADR-001-row-encoding-and-catalog.md`.

mod catalog;
mod codec;

pub use catalog::{Catalog, CatalogError, TableHandle};
pub use codec::{decode_table_entry, encode_table_entry, CatalogCodecError};
