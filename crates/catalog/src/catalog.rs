//! The catalog: table schemas stored as ordinary rows in the same
//! `sietch::Store` as everything else, under the reserved catalog
//! namespace (`row::CATALOG_NAMESPACE`) — so `CREATE TABLE` gets exactly
//! the same crash-safety and append-only versioning as data, with no
//! separate metadata file to keep consistent.

use std::path::PathBuf;

use row::{catalog_counter_key, catalog_table_key, ColumnDef, Schema, SchemaError};
use storage::{Store, StoreError};

use crate::codec::{decode_table_entry, encode_table_entry, CatalogCodecError};

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
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error(transparent)]
    Codec(#[from] CatalogCodecError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct Catalog {
    store: Store,
}

impl Catalog {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        Ok(Self {
            store: Store::open(dir)?,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    fn next_table_id(&mut self) -> Result<u32, CatalogError> {
        let key = catalog_counter_key();
        let current = match self.store.get(&key) {
            Some(bytes) if bytes.len() == 4 => u32::from_le_bytes(bytes.try_into().unwrap()),
            _ => 0,
        };
        self.store.put(key, (current + 1).to_le_bytes().to_vec())?;
        Ok(current)
    }

    /// Creates a new table, assigning it a fresh, never-reused table id.
    /// Fails if the name is empty, already used, or the schema itself is
    /// invalid (see `row::Schema::new`'s rules — no composite primary
    /// keys, primary key never nullable, no duplicate column names).
    pub fn create_table(
        &mut self,
        name: impl Into<String>,
        columns: Vec<ColumnDef>,
        primary_key: usize,
    ) -> Result<TableHandle, CatalogError> {
        let name = name.into();
        if name.is_empty() {
            return Err(CatalogError::EmptyTableName);
        }
        let schema = Schema::new(name.clone(), columns, primary_key)?;
        let key = catalog_table_key(&name).expect("just checked non-empty");
        if self.store.get(&key).is_some() {
            return Err(CatalogError::TableAlreadyExists(name));
        }
        let table_id = self.next_table_id()?;
        self.store.put(key, encode_table_entry(table_id, &schema))?;
        Ok(TableHandle { table_id, schema })
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

    /// Every table currently defined, in no particular order.
    pub fn list_tables(&self) -> Result<Vec<TableHandle>, CatalogError> {
        let counter_key = catalog_counter_key();
        let mut tables = Vec::new();
        for (key, value) in self.store.scan(&[row::CATALOG_NAMESPACE]) {
            if key == counter_key {
                continue; // the counter itself, not a table entry
            }
            let (table_id, schema) = decode_table_entry(&value)?;
            tables.push(TableHandle { table_id, schema });
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
        let mut catalog = Catalog::open(dir.path()).unwrap();
        let created = catalog.create_table("users", cols(), 0).unwrap();
        let fetched = catalog.get_table("users").unwrap().unwrap();
        assert_eq!(created, fetched);
    }

    #[test]
    fn table_ids_are_assigned_uniquely_and_increasing() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(dir.path()).unwrap();
        let a = catalog.create_table("a", cols(), 0).unwrap();
        let b = catalog.create_table("b", cols(), 0).unwrap();
        let c = catalog.create_table("c", cols(), 0).unwrap();
        assert_eq!([a.table_id, b.table_id, c.table_id], [0, 1, 2]);
    }

    #[test]
    fn duplicate_table_name_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(dir.path()).unwrap();
        catalog.create_table("users", cols(), 0).unwrap();
        assert!(matches!(
            catalog.create_table("users", cols(), 0),
            Err(CatalogError::TableAlreadyExists(_))
        ));
    }

    #[test]
    fn empty_table_name_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(dir.path()).unwrap();
        assert!(matches!(
            catalog.create_table("", cols(), 0),
            Err(CatalogError::EmptyTableName)
        ));
    }

    #[test]
    fn an_invalid_schema_is_rejected_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(dir.path()).unwrap();
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
    fn list_tables_excludes_the_counter_and_includes_every_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(dir.path()).unwrap();
        catalog.create_table("a", cols(), 0).unwrap();
        catalog.create_table("b", cols(), 0).unwrap();
        let mut names: Vec<String> = catalog
            .list_tables()
            .unwrap()
            .into_iter()
            .map(|t| t.schema.table_name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn the_catalog_survives_a_real_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut catalog = Catalog::open(dir.path()).unwrap();
            catalog.create_table("users", cols(), 0).unwrap();
        }
        let catalog = Catalog::open(dir.path()).unwrap();
        let fetched = catalog.get_table("users").unwrap().unwrap();
        assert_eq!(fetched.schema.table_name, "users");
        assert_eq!(fetched.table_id, 0);
    }

    #[test]
    fn a_table_id_is_never_reused_even_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut catalog = Catalog::open(dir.path()).unwrap();
            catalog.create_table("a", cols(), 0).unwrap();
        }
        let mut catalog = Catalog::open(dir.path()).unwrap();
        let b = catalog.create_table("b", cols(), 0).unwrap();
        assert_eq!(b.table_id, 1);
    }
}
