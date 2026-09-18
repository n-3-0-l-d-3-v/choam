//! The relational transaction API (ticket 002): `Database::begin` starts
//! a `RelTransaction` that reads and writes whole rows by table name and
//! primary key, buffered exactly like `sietch::Transaction` underneath,
//! and committed or aborted as one atomic unit — multiple statements
//! against multiple tables all-or-nothing.
//!
//! This crate adds no isolation semantics of its own: a `RelTransaction`
//! is a thin, row/schema-aware wrapper over exactly one
//! `sietch::Transaction`, so it inherits Snapshot Isolation and
//! first-committer-wins conflict detection unchanged. What this crate
//! *does* add is turning a raw `TxnError::Conflict(Vec<u8>)` — a byte key
//! meaningless outside sietch — into a `DbError::Conflict` naming the
//! actual table and primary key it happened on, via `row::split_row_key`/
//! `decode_key_component`.
//!
//! `Database` and `Catalog` share exactly one `TransactionalStore`
//! (`Catalog::store()`), never two separate opens of the same directory —
//! see `docs/design/decisions/ADR-002-relational-transactions.md`.

use catalog::{Catalog, CatalogError, TableHandle};
use row::{
    decode_key_component, decode_row, encode_row, row_key, split_row_key, RowError, Value,
    ValueError,
};
use storage::TxnError;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("unknown table {0:?}")]
    UnknownTable(String),
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error(transparent)]
    Row(#[from] RowError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Store(#[from] storage::StoreError),
    /// Another transaction committed a conflicting write to this exact
    /// row first (first-committer-wins) — the whole transaction was
    /// aborted; the caller can retry it from scratch.
    #[error("transaction conflict on table {table:?} row {pk:?} — retry the transaction")]
    Conflict { table: String, pk: Value },
    /// The conflicting key couldn't be attributed to any known table/row.
    /// Not reachable in this ticket's scope (there is no DROP TABLE to
    /// make a table id go stale), kept as a safe fallback rather than a
    /// panic.
    #[error("transaction conflict on an unrecognized key {0:?}")]
    UnrecognizedConflict(Vec<u8>),
}

/// The relational database: catalog operations plus `begin` for
/// multi-statement transactions against row data.
pub struct Database {
    catalog: Catalog,
}

impl Database {
    pub fn open(dir: impl Into<std::path::PathBuf>) -> Result<Self, DbError> {
        Ok(Self {
            catalog: Catalog::open(dir)?,
        })
    }

    pub fn create_table(
        &self,
        name: impl Into<String>,
        columns: Vec<row::ColumnDef>,
        primary_key: usize,
    ) -> Result<TableHandle, DbError> {
        Ok(self.catalog.create_table(name, columns, primary_key)?)
    }

    pub fn get_table(&self, name: &str) -> Result<Option<TableHandle>, DbError> {
        Ok(self.catalog.get_table(name)?)
    }

    pub fn list_tables(&self) -> Result<Vec<TableHandle>, DbError> {
        Ok(self.catalog.list_tables()?)
    }

    /// Starts a new relational transaction on a snapshot taken right now.
    pub fn begin(&self) -> RelTransaction<'_> {
        RelTransaction {
            inner: self.catalog.store().begin(),
            db: self,
        }
    }

    fn table_handle(&self, name: &str) -> Result<TableHandle, DbError> {
        self.catalog
            .get_table(name)?
            .ok_or_else(|| DbError::UnknownTable(name.to_string()))
    }

    /// Turns a raw `TxnError::Conflict` key back into the table name and
    /// primary key value it belongs to, by matching the key's table id
    /// against `list_tables()` and decoding the rest as that table's
    /// primary-key type.
    fn conflict_error(&self, key: &[u8]) -> DbError {
        let Some((table_id, pk_bytes)) = split_row_key(key) else {
            return DbError::UnrecognizedConflict(key.to_vec());
        };
        let Ok(tables) = self.catalog.list_tables() else {
            return DbError::UnrecognizedConflict(key.to_vec());
        };
        let Some(handle) = tables.into_iter().find(|h| h.table_id == table_id) else {
            return DbError::UnrecognizedConflict(key.to_vec());
        };
        match decode_key_component(pk_bytes, handle.schema.primary_key_type()) {
            Ok(pk) => DbError::Conflict {
                table: handle.schema.table_name,
                pk,
            },
            Err(_) => DbError::UnrecognizedConflict(key.to_vec()),
        }
    }
}

/// A buffered, multi-statement, multi-table transaction against row data.
/// Nothing it reads or writes is visible to anyone else until `commit()`
/// succeeds; `abort()` (or simply dropping it) discards it entirely.
pub struct RelTransaction<'db> {
    inner: storage::Transaction,
    db: &'db Database,
}

impl RelTransaction<'_> {
    /// Reads one row by primary key, as this transaction would see it:
    /// its own uncommitted write if it has one, otherwise the value as of
    /// this transaction's snapshot.
    pub fn read(&self, table: &str, pk: &Value) -> Result<Option<Vec<Value>>, DbError> {
        let handle = self.db.table_handle(table)?;
        let key = row_key(handle.table_id, pk, handle.schema.primary_key_type())?;
        match self.inner.get(&key) {
            None => Ok(None),
            Some(bytes) => Ok(Some(decode_row(&handle.schema, &bytes)?)),
        }
    }

    /// Buffers a whole-row write (insert or overwrite), keyed by
    /// `values`'s primary-key column.
    pub fn write(&mut self, table: &str, values: &[Value]) -> Result<(), DbError> {
        let handle = self.db.table_handle(table)?;
        let pk = &values[handle.schema.primary_key];
        let key = row_key(handle.table_id, pk, handle.schema.primary_key_type())?;
        let encoded = encode_row(&handle.schema, values)?;
        self.inner.put(key, encoded);
        Ok(())
    }

    /// Buffers a row deletion by primary key.
    pub fn delete(&mut self, table: &str, pk: &Value) -> Result<(), DbError> {
        let handle = self.db.table_handle(table)?;
        let key = row_key(handle.table_id, pk, handle.schema.primary_key_type())?;
        self.inner.delete(key);
        Ok(())
    }

    /// Commits every buffered write as one atomic, all-or-nothing unit.
    /// On a write-write conflict with another transaction that committed
    /// first, nothing this transaction wrote is applied, and the error
    /// names the exact table and row that conflicted.
    pub fn commit(self) -> Result<(), DbError> {
        let db = self.db;
        match self.inner.commit() {
            Ok(()) => Ok(()),
            Err(TxnError::Conflict(key)) => Err(db.conflict_error(&key)),
            Err(TxnError::Store(e)) => Err(DbError::Store(e)),
        }
    }

    /// Discards every buffered write. Equivalent to dropping the
    /// transaction; provided as an explicit, readable alternative.
    pub fn abort(self) {
        self.inner.abort()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use row::ColumnType;

    fn cols() -> Vec<row::ColumnDef> {
        vec![
            row::ColumnDef {
                name: "id".into(),
                ty: ColumnType::Integer,
                nullable: false,
            },
            row::ColumnDef {
                name: "name".into(),
                ty: ColumnType::Text,
                nullable: true,
            },
        ]
    }

    fn open_with_users() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).unwrap();
        db.create_table("users", cols(), 0).unwrap();
        (dir, db)
    }

    #[test]
    fn write_then_read_in_the_same_transaction_sees_it() {
        let (_dir, db) = open_with_users();
        let mut txn = db.begin();
        txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
            .unwrap();
        let row = txn.read("users", &Value::Integer(1)).unwrap().unwrap();
        assert_eq!(row, vec![Value::Integer(1), Value::Text("alice".into())]);
    }

    #[test]
    fn an_uncommitted_write_is_invisible_to_another_transaction() {
        let (_dir, db) = open_with_users();
        let mut txn = db.begin();
        txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
            .unwrap();

        let other = db.begin();
        assert_eq!(other.read("users", &Value::Integer(1)).unwrap(), None);

        txn.commit().unwrap();
        let after = db.begin();
        assert_eq!(
            after.read("users", &Value::Integer(1)).unwrap().unwrap(),
            vec![Value::Integer(1), Value::Text("alice".into())]
        );
    }

    #[test]
    fn multiple_rows_in_multiple_tables_commit_as_one_unit() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).unwrap();
        db.create_table("users", cols(), 0).unwrap();
        db.create_table("orders", cols(), 0).unwrap();

        let mut txn = db.begin();
        txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
            .unwrap();
        txn.write("orders", &[Value::Integer(1), Value::Text("widget".into())])
            .unwrap();
        txn.commit().unwrap();

        let readback = db.begin();
        assert!(readback
            .read("users", &Value::Integer(1))
            .unwrap()
            .is_some());
        assert!(readback
            .read("orders", &Value::Integer(1))
            .unwrap()
            .is_some());
    }

    #[test]
    fn abort_discards_every_buffered_write() {
        let (_dir, db) = open_with_users();
        let mut txn = db.begin();
        txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
            .unwrap();
        txn.abort();

        let after = db.begin();
        assert_eq!(after.read("users", &Value::Integer(1)).unwrap(), None);
    }

    #[test]
    fn delete_removes_a_previously_committed_row() {
        let (_dir, db) = open_with_users();
        {
            let mut txn = db.begin();
            txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
                .unwrap();
            txn.commit().unwrap();
        }
        {
            let mut txn = db.begin();
            txn.delete("users", &Value::Integer(1)).unwrap();
            txn.commit().unwrap();
        }
        let after = db.begin();
        assert_eq!(after.read("users", &Value::Integer(1)).unwrap(), None);
    }

    #[test]
    fn writing_to_an_unknown_table_is_rejected() {
        let (_dir, db) = open_with_users();
        let mut txn = db.begin();
        assert!(matches!(
            txn.write("ghosts", &[Value::Integer(1)]),
            Err(DbError::UnknownTable(name)) if name == "ghosts"
        ));
    }

    #[test]
    fn a_write_write_conflict_is_reported_with_the_table_and_primary_key() {
        let (_dir, db) = open_with_users();
        let mut first = db.begin();
        let mut second = db.begin();
        first
            .write("users", &[Value::Integer(1), Value::Text("alice".into())])
            .unwrap();
        second
            .write("users", &[Value::Integer(1), Value::Text("bob".into())])
            .unwrap();

        first.commit().unwrap();
        let err = second.commit().unwrap_err();
        match err {
            DbError::Conflict { table, pk } => {
                assert_eq!(table, "users");
                assert_eq!(pk, Value::Integer(1));
            }
            other => panic!("expected a Conflict error, got {other:?}"),
        }
    }

    #[test]
    fn a_conflict_on_one_row_aborts_the_whole_transaction() {
        let (_dir, db) = open_with_users();
        {
            let mut txn = db.begin();
            txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
                .unwrap();
            txn.commit().unwrap();
        }

        let mut first = db.begin();
        let mut second = db.begin();
        first
            .write("users", &[Value::Integer(1), Value::Text("changed".into())])
            .unwrap();
        first.commit().unwrap();

        second
            .write(
                "users",
                &[Value::Integer(1), Value::Text("conflict".into())],
            )
            .unwrap();
        second
            .write(
                "users",
                &[Value::Integer(2), Value::Text("should not apply".into())],
            )
            .unwrap();
        assert!(second.commit().is_err());

        let after = db.begin();
        assert_eq!(after.read("users", &Value::Integer(2)).unwrap(), None);
    }

    #[test]
    fn a_transactions_reads_are_pinned_to_its_snapshot() {
        let (_dir, db) = open_with_users();
        {
            let mut txn = db.begin();
            txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
                .unwrap();
            txn.commit().unwrap();
        }

        let reader = db.begin();
        assert_eq!(
            reader.read("users", &Value::Integer(1)).unwrap().unwrap(),
            vec![Value::Integer(1), Value::Text("alice".into())]
        );

        let mut writer = db.begin();
        writer
            .write("users", &[Value::Integer(1), Value::Text("bob".into())])
            .unwrap();
        writer.commit().unwrap();

        assert_eq!(
            reader.read("users", &Value::Integer(1)).unwrap().unwrap(),
            vec![Value::Integer(1), Value::Text("alice".into())],
            "a transaction's reads must never see writes committed after its snapshot"
        );
    }

    #[test]
    fn the_database_survives_a_real_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Database::open(dir.path()).unwrap();
            db.create_table("users", cols(), 0).unwrap();
            let mut txn = db.begin();
            txn.write("users", &[Value::Integer(1), Value::Text("alice".into())])
                .unwrap();
            txn.commit().unwrap();
        }
        let db = Database::open(dir.path()).unwrap();
        let txn = db.begin();
        assert_eq!(
            txn.read("users", &Value::Integer(1)).unwrap().unwrap(),
            vec![Value::Integer(1), Value::Text("alice".into())]
        );
    }
}
