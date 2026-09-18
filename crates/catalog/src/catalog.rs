//! The catalog: table schemas stored as ordinary rows in the same
//! `sietch::TransactionalStore` as everything else (ticket 002 moved
//! this off plain `Store`, so the catalog and relational row-data
//! transactions share one thread-safe handle to the one underlying
//! store — see `docs/design/decisions/ADR-002-relational-transactions.md`),
//! under the reserved catalog namespace (`row::CATALOG_NAMESPACE`) — so
//! `CREATE TABLE` gets exactly the same crash-safety and versioning as
//! data, with no separate metadata file to keep consistent.
//!
//! `TransactionalStore` exposes no `scan`, only `get`/transactions, so
//! table names are tracked in an explicit index entry
//! (`row::catalog_index_key`) kept in sync with each table's own entry
//! **in the same transaction** that creates it — the index can never
//! drift from the entries it lists.

use std::path::PathBuf;

use row::{
    catalog_counter_key, catalog_index_key, catalog_table_key, ColumnDef, Schema, SchemaError,
};
use storage::{StoreError, TransactionalStore, TxnError};

use crate::codec::{
    decode_table_entry, decode_table_index, encode_table_entry, encode_table_index,
    CatalogCodecError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableHandle {
    pub table_id: u32,
    pub schema: Schema,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("table name must not be empty")]
    EmptyTableName,
    #[error("table {0:?} already exists")]
    TableAlreadyExists(String),
    /// A concurrent `create_table` raced this one for the shared
    /// counter/index entries and committed first — the caller can simply
    /// retry (a fresh attempt reads the now-updated counter/index).
    #[error("concurrent CREATE TABLE conflict — retry the call")]
    ConcurrentCreateTable,
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Codec(#[from] CatalogCodecError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct Catalog {
    store: TransactionalStore,
}

impl Catalog {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        Ok(Self {
            store: TransactionalStore::open(dir)?,
        })
    }

    /// The shared, thread-safe store handle — also used by `txn`'s
    /// relational transactions, so both go through the exact same
    /// `sietch` instance rather than two independent (and therefore
    /// unsafe-to-share) opens of the same directory.
    pub fn store(&self) -> &TransactionalStore {
        &self.store
    }

    /// Creates a new table, assigning it a fresh, never-reused table id.
    /// Fails if the name is empty, already used, or the schema itself is
    /// invalid (see `row::Schema::new`'s rules — no composite primary
    /// keys, primary key never nullable, no duplicate column names).
    /// Concurrent `create_table` calls are safe: they race on the shared
    /// counter/index keys, and the loser gets `ConcurrentCreateTable`
    /// rather than corrupting either.
    pub fn create_table(
        &self,
        name: impl Into<String>,
        columns: Vec<ColumnDef>,
        primary_key: usize,
    ) -> Result<TableHandle, CatalogError> {
        let name = name.into();
        if name.is_empty() {
            return Err(CatalogError::EmptyTableName);
        }
        let schema = Schema::new(name.clone(), columns, primary_key)?;
        let table_key = catalog_table_key(&name).expect("just checked non-empty");

        let mut txn = self.store.begin();
        if txn.get(&table_key).is_some() {
            return Err(CatalogError::TableAlreadyExists(name));
        }

        let counter_key = catalog_counter_key();
        let table_id = match txn.get(&counter_key) {
            Some(bytes) if bytes.len() == 4 => u32::from_le_bytes(bytes.try_into().unwrap()),
            _ => 0,
        };
        txn.put(counter_key, (table_id + 1).to_le_bytes().to_vec());
        txn.put(table_key, encode_table_entry(table_id, &schema));

        let index_key = catalog_index_key();
        let mut names = match txn.get(&index_key) {
            Some(bytes) => decode_table_index(&bytes)?,
            None => Vec::new(),
        };
        names.push(name);
        txn.put(index_key, encode_table_index(&names));

        match txn.commit() {
            Ok(()) => Ok(TableHandle { table_id, schema }),
            Err(TxnError::Conflict(_)) => Err(CatalogError::ConcurrentCreateTable),
            Err(TxnError::Store(e)) => Err(CatalogError::Store(e)),
        }
    }

    pub fn get_table(&self, name: &str) -> Result<Option<TableHandle>, CatalogError> {
        let Some(key) = catalog_table_key(name) else {
            return Ok(None); // an empty name can never be a real table
        };
        match self.store.get(&key) {
            None => Ok(None),
            Some(bytes) => {
                let (table_id, schema) = decode_table_entry(&bytes)?;
                Ok(Some(TableHandle { table_id, schema }))
            }
        }
    }

    /// Every table currently defined, in the order they were created.
    pub fn list_tables(&self) -> Result<Vec<TableHandle>, CatalogError> {
        let names = match self.store.get(&catalog_index_key()) {
            Some(bytes) => decode_table_index(&bytes)?,
            None => Vec::new(),
        };
        let mut tables = Vec::with_capacity(names.len());
        for name in names {
            if let Some(handle) = self.get_table(&name)? {
                tables.push(handle);
            }
        }
        Ok(tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use row::ColumnType;

    fn cols() -> Vec<ColumnDef> {
        vec![
            ColumnDef {
                name: "id".into(),
                ty: ColumnType::Integer,
                nullable: false,
            },
            ColumnDef {
                name: "name".into(),
                ty: ColumnType::Text,
                nullable: true,
            },
        ]
    }

    #[test]
    fn create_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        let created = catalog.create_table("users", cols(), 0).unwrap();
        let fetched = catalog.get_table("users").unwrap().unwrap();
        assert_eq!(created, fetched);
    }

    #[test]
    fn table_ids_are_assigned_uniquely_and_increasing() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        let a = catalog.create_table("a", cols(), 0).unwrap();
        let b = catalog.create_table("b", cols(), 0).unwrap();
        let c = catalog.create_table("c", cols(), 0).unwrap();
        assert_eq!([a.table_id, b.table_id, c.table_id], [0, 1, 2]);
    }

    #[test]
    fn duplicate_table_name_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        catalog.create_table("users", cols(), 0).unwrap();
        assert!(matches!(
            catalog.create_table("users", cols(), 0),
            Err(CatalogError::TableAlreadyExists(_))
        ));
    }

    #[test]
    fn empty_table_name_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        assert!(matches!(
            catalog.create_table("", cols(), 0),
            Err(CatalogError::EmptyTableName)
        ));
    }

    #[test]
    fn an_invalid_schema_is_rejected_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        assert!(catalog.create_table("t", vec![], 0).is_err());
        assert_eq!(catalog.list_tables().unwrap().len(), 0);
        // The table-id counter must not have been consumed either.
        let ok = catalog.create_table("t", cols(), 0).unwrap();
        assert_eq!(ok.table_id, 0);
    }

    #[test]
    fn get_unknown_table_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        assert_eq!(catalog.get_table("nope").unwrap(), None);
    }

    #[test]
    fn list_tables_includes_every_table_in_creation_order() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        catalog.create_table("b", cols(), 0).unwrap();
        catalog.create_table("a", cols(), 0).unwrap();
        let names: Vec<String> = catalog
            .list_tables()
            .unwrap()
            .into_iter()
            .map(|t| t.schema.table_name)
            .collect();
        assert_eq!(names, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn the_catalog_survives_a_real_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let catalog = Catalog::open(dir.path()).unwrap();
            catalog.create_table("users", cols(), 0).unwrap();
        }
        let catalog = Catalog::open(dir.path()).unwrap();
        let fetched = catalog.get_table("users").unwrap().unwrap();
        assert_eq!(fetched.schema.table_name, "users");
        assert_eq!(fetched.table_id, 0);
        assert_eq!(catalog.list_tables().unwrap().len(), 1);
    }

    #[test]
    fn a_table_id_is_never_reused_even_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let catalog = Catalog::open(dir.path()).unwrap();
            catalog.create_table("a", cols(), 0).unwrap();
        }
        let catalog = Catalog::open(dir.path()).unwrap();
        let b = catalog.create_table("b", cols(), 0).unwrap();
        assert_eq!(b.table_id, 1);
    }

    #[test]
    fn concurrent_create_table_calls_never_corrupt_the_counter_or_index() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(Catalog::open(dir.path()).unwrap());
        let n = 8;
        let barrier = Arc::new(Barrier::new(n));
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let catalog = catalog.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    let name = format!("t{i}");
                    // Retry on the (expected, safe) concurrent-create conflict.
                    loop {
                        match catalog.create_table(name.clone(), cols(), 0) {
                            Ok(handle) => return handle,
                            Err(CatalogError::ConcurrentCreateTable) => continue,
                            Err(e) => panic!("unexpected error: {e}"),
                        }
                    }
                })
            })
            .collect();
        let results: Vec<TableHandle> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let mut ids: Vec<u32> = results.iter().map(|h| h.table_id).collect();
        ids.sort();
        assert_eq!(
            ids,
            (0..n as u32).collect::<Vec<_>>(),
            "every table id 0..n must be used exactly once"
        );

        let tables = catalog.list_tables().unwrap();
        assert_eq!(
            tables.len(),
            n,
            "the index must list exactly n tables, no duplicates or losses"
        );
    }
}
