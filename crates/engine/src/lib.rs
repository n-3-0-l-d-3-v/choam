//! SQL execution over `txn` (ticket 004): a `Session` runs parsed
//! statements either in autocommit mode (each statement its own
//! transaction) or inside an explicit BEGIN ... COMMIT block.
//!
//! Semantics worth knowing:
//! - A statement that fails inside an explicit transaction poisons it
//!   (PostgreSQL-style): later statements are refused until ROLLBACK (or
//!   COMMIT, which rolls back and reports the abort). `sietch::Transaction`
//!   has no savepoints, so a half-applied statement (say, a multi-row
//!   INSERT that failed on row 3) cannot be undone in place.
//! - DDL (CREATE TABLE) is not allowed inside a transaction.
//! - UPDATE may not change a primary-key column.
//! - The only access paths are a point lookup (WHERE contains
//!   `pk = <constant>`) and a full table scan; see `access_path`.
//!
//! See `docs/design/decisions/ADR-004-sql-executor.md`.

mod eval;

use std::cmp::Ordering;

use row::{ColumnDef, ColumnType, Schema, Value};
use sql::{parse_script, Expr, OrderBy, ParseError, Projection, Statement};
use txn::{Database, DbError, RelTransaction};

use eval::{check_filter, check_type, compare, eval, keeps_row, Ty};

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("unknown column {0:?}")]
    UnknownColumn(String),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("division by zero")]
    DivisionByZero,
    #[error("integer overflow")]
    IntegerOverflow,
    #[error("table must declare exactly one PRIMARY KEY column")]
    PrimaryKeyCount,
    #[error("primary key value must not be NULL")]
    NullPrimaryKey,
    #[error("duplicate primary key {0:?}")]
    DuplicatePrimaryKey(Value),
    #[error("column {0:?} listed twice")]
    DuplicateColumn(String),
    #[error("INSERT row has {found} values but {expected} columns were targeted")]
    ValueCount { expected: usize, found: usize },
    #[error("UPDATE cannot change primary key column {0:?}")]
    UpdatePrimaryKey(String),
    #[error("CREATE TABLE is not allowed inside a transaction")]
    DdlInTransaction,
    #[error("already inside a transaction")]
    AlreadyInTransaction,
    #[error("no transaction is active")]
    NoActiveTransaction,
    #[error("current transaction is aborted; ROLLBACK to continue")]
    TransactionAborted,
}

#[cfg(test)]
impl PartialEq for ExecError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

/// The outcome of one statement. `columns`/`rows` are filled only by
/// SELECT; `rows_affected` counts rows inserted, updated or deleted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub rows_affected: usize,
}

/// How a statement will find its rows.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessPath {
    /// The WHERE clause pins the primary key to one constant.
    PointLookup(Value),
    FullScan,
}

/// Chooses an access path: a point lookup if the filter is, or is an AND
/// chain containing, `pk = <constant>` (either operand order) with a
/// constant of the primary key's own type; otherwise a full scan. The full
/// filter is still applied to whatever rows the path returns.
pub fn access_path(schema: &Schema, filter: Option<&Expr>) -> AccessPath {
    fn find(schema: &Schema, e: &Expr) -> Option<Value> {
        use sql::BinaryOp;
        let Expr::Binary(l, op, r) = e else {
            return None;
        };
        match op {
            BinaryOp::And => find(schema, l).or_else(|| find(schema, r)),
            BinaryOp::Eq => {
                let pk = &schema.columns[schema.primary_key].name;
                let other = match (&**l, &**r) {
                    (Expr::Column(c), o) | (o, Expr::Column(c)) if c == pk => o,
                    _ => return None,
                };
                let v = eval(other, None).ok()?;
                let ok = matches!(
                    (&v, schema.primary_key_type()),
                    (Value::Integer(_), ColumnType::Integer)
                        | (Value::Text(_), ColumnType::Text)
                        | (Value::Boolean(_), ColumnType::Boolean)
                );
                ok.then_some(v)
            }
            _ => None,
        }
    }
    filter
        .and_then(|f| find(schema, f))
        .map_or(AccessPath::FullScan, AccessPath::PointLookup)
}

/// A database plus the ability to open sessions on it.
pub struct Engine {
    db: Database,
}

impl Engine {
    pub fn open(dir: impl Into<std::path::PathBuf>) -> Result<Self, ExecError> {
        Ok(Self {
            db: Database::open(dir)?,
        })
    }

    pub fn session(&self) -> Session<'_> {
        Session {
            db: &self.db,
            txn: None,
            failed: false,
        }
    }

    pub fn database(&self) -> &Database {
        &self.db
    }
}

pub struct Session<'db> {
    db: &'db Database,
    txn: Option<RelTransaction<'db>>,
    failed: bool,
}

impl<'db> Session<'db> {
    pub fn in_transaction(&self) -> bool {
        self.txn.is_some()
    }

    /// Parses and runs every `;`-separated statement, returning each
    /// result, stopping at the first error.
    pub fn execute_script(&mut self, sql: &str) -> Result<Vec<QueryResult>, ExecError> {
        let stmts = parse_script(sql)?;
        stmts.iter().map(|s| self.execute_statement(s)).collect()
    }

    /// Parses and runs exactly one statement.
    pub fn execute(&mut self, sql: &str) -> Result<QueryResult, ExecError> {
        let stmt = sql::parse(sql)?;
        self.execute_statement(&stmt)
    }

    pub fn execute_statement(&mut self, stmt: &Statement) -> Result<QueryResult, ExecError> {
        match stmt {
            Statement::Begin => {
                if self.txn.is_some() {
                    return Err(ExecError::AlreadyInTransaction);
                }
                self.txn = Some(self.db.begin());
                Ok(QueryResult::default())
            }
            Statement::Commit => {
                let txn = self.txn.take().ok_or(ExecError::NoActiveTransaction)?;
                if std::mem::take(&mut self.failed) {
                    txn.abort();
                    return Err(ExecError::TransactionAborted);
                }
                txn.commit()?;
                Ok(QueryResult::default())
            }
            Statement::Rollback => {
                let txn = self.txn.take().ok_or(ExecError::NoActiveTransaction)?;
                self.failed = false;
                txn.abort();
                Ok(QueryResult::default())
            }
            Statement::CreateTable { name, columns } => {
                if self.txn.is_some() {
                    return Err(ExecError::DdlInTransaction);
                }
                create_table(self.db, name, columns)
            }
            other => {
                if self.failed {
                    return Err(ExecError::TransactionAborted);
                }
                if let Some(txn) = self.txn.as_mut() {
                    let r = run(self.db, txn, other);
                    self.failed = r.is_err();
                    r
                } else {
                    let mut txn = self.db.begin();
                    let r = run(self.db, &mut txn, other)?;
                    txn.commit()?;
                    Ok(r)
                }
            }
        }
    }
}

fn create_table(
    db: &Database,
    name: &str,
    columns: &[sql::ColumnSpec],
) -> Result<QueryResult, ExecError> {
    let pks: Vec<usize> = columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.primary_key.then_some(i))
        .collect();
    let [pk] = pks[..] else {
        return Err(ExecError::PrimaryKeyCount);
    };
    let defs = columns
        .iter()
        .enumerate()
        .map(|(i, c)| ColumnDef {
            name: c.name.clone(),
            ty: c.ty,
            nullable: !c.not_null && i != pk,
        })
        .collect();
    db.create_table(name, defs, pk)?;
    Ok(QueryResult::default())
}

fn schema_of(db: &Database, table: &str) -> Result<Schema, ExecError> {
    Ok(db
        .get_table(table)?
        .ok_or_else(|| DbError::UnknownTable(table.to_string()))?
        .schema)
}

fn column_index(schema: &Schema, name: &str) -> Result<usize, ExecError> {
    schema
        .columns
        .iter()
        .position(|c| c.name == name)
        .ok_or_else(|| ExecError::UnknownColumn(name.to_string()))
}

fn candidate_rows(
    txn: &RelTransaction<'_>,
    table: &str,
    schema: &Schema,
    filter: Option<&Expr>,
) -> Result<Vec<Vec<Value>>, ExecError> {
    check_filter(filter, schema)?;
    Ok(match access_path(schema, filter) {
        AccessPath::PointLookup(pk) => txn.read(table, &pk)?.into_iter().collect(),
        AccessPath::FullScan => txn.scan(table)?,
    })
}

fn matching_rows(
    txn: &RelTransaction<'_>,
    table: &str,
    schema: &Schema,
    filter: Option<&Expr>,
) -> Result<Vec<Vec<Value>>, ExecError> {
    let mut out = Vec::new();
    for row in candidate_rows(txn, table, schema, filter)? {
        if keeps_row(filter, schema, &row)? {
            out.push(row);
        }
    }
    Ok(out)
}

fn run(
    db: &Database,
    txn: &mut RelTransaction<'_>,
    stmt: &Statement,
) -> Result<QueryResult, ExecError> {
    match stmt {
        Statement::Insert {
            table,
            columns,
            rows,
        } => insert(db, txn, table, columns.as_deref(), rows),
        Statement::Select {
            projection,
            table,
            filter,
            order_by,
            limit,
        } => select(
            db,
            txn,
            table,
            projection,
            filter.as_ref(),
            order_by.as_ref(),
            *limit,
        ),
        Statement::Update {
            table,
            assignments,
            filter,
        } => update(db, txn, table, assignments, filter.as_ref()),
        Statement::Delete { table, filter } => {
            let schema = schema_of(db, table)?;
            let victims = matching_rows(txn, table, &schema, filter.as_ref())?;
            for row in &victims {
                txn.delete(table, &row[schema.primary_key])?;
            }
            Ok(QueryResult {
                rows_affected: victims.len(),
                ..Default::default()
            })
        }
        _ => unreachable!("transaction control and DDL are handled by Session"),
    }
}

fn insert(
    db: &Database,
    txn: &mut RelTransaction<'_>,
    table: &str,
    columns: Option<&[String]>,
    rows: &[Vec<Expr>],
) -> Result<QueryResult, ExecError> {
    let schema = schema_of(db, table)?;
    let targets: Vec<usize> = match columns {
        None => (0..schema.columns.len()).collect(),
        Some(names) => {
            let mut idx = Vec::new();
            for n in names {
                let i = column_index(&schema, n)?;
                if idx.contains(&i) {
                    return Err(ExecError::DuplicateColumn(n.clone()));
                }
                idx.push(i);
            }
            idx
        }
    };
    for exprs in rows {
        if exprs.len() != targets.len() {
            return Err(ExecError::ValueCount {
                expected: targets.len(),
                found: exprs.len(),
            });
        }
        let mut values = vec![Value::Null; schema.columns.len()];
        for (&i, e) in targets.iter().zip(exprs) {
            values[i] = eval(e, None)?;
        }
        let pk = &values[schema.primary_key];
        if matches!(pk, Value::Null) {
            return Err(ExecError::NullPrimaryKey);
        }
        if txn.read(table, pk)?.is_some() {
            return Err(ExecError::DuplicatePrimaryKey(pk.clone()));
        }
        txn.write(table, &values)?;
    }
    Ok(QueryResult {
        rows_affected: rows.len(),
        ..Default::default()
    })
}

fn select(
    db: &Database,
    txn: &RelTransaction<'_>,
    table: &str,
    projection: &Projection,
    filter: Option<&Expr>,
    order_by: Option<&OrderBy>,
    limit: Option<u64>,
) -> Result<QueryResult, ExecError> {
    let schema = schema_of(db, table)?;
    let picked: Vec<usize> = match projection {
        Projection::All => (0..schema.columns.len()).collect(),
        Projection::Columns(names) => names
            .iter()
            .map(|n| column_index(&schema, n))
            .collect::<Result<_, _>>()?,
    };
    let order = order_by
        .map(|o| Ok::<_, ExecError>((column_index(&schema, &o.column)?, o.descending)))
        .transpose()?;

    let mut rows = matching_rows(txn, table, &schema, filter)?;
    if let Some((i, desc)) = order {
        // NULLs sort first ascending (last descending); the sort is stable.
        rows.sort_by(|a, b| {
            let ord = match (&a[i], &b[i]) {
                (Value::Null, Value::Null) => Ordering::Equal,
                (Value::Null, _) => Ordering::Less,
                (_, Value::Null) => Ordering::Greater,
                (x, y) => compare(x, y).unwrap_or(Ordering::Equal),
            };
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
    if let Some(n) = limit {
        rows.truncate(usize::try_from(n).unwrap_or(usize::MAX));
    }
    Ok(QueryResult {
        columns: picked
            .iter()
            .map(|&i| schema.columns[i].name.clone())
            .collect(),
        rows: rows
            .into_iter()
            .map(|r| picked.iter().map(|&i| r[i].clone()).collect())
            .collect(),
        rows_affected: 0,
    })
}

fn update(
    db: &Database,
    txn: &mut RelTransaction<'_>,
    table: &str,
    assignments: &[(String, Expr)],
    filter: Option<&Expr>,
) -> Result<QueryResult, ExecError> {
    let schema = schema_of(db, table)?;
    let mut targets = Vec::new();
    for (name, e) in assignments {
        let i = column_index(&schema, name)?;
        if i == schema.primary_key {
            return Err(ExecError::UpdatePrimaryKey(name.clone()));
        }
        if targets.iter().any(|(j, _)| *j == i) {
            return Err(ExecError::DuplicateColumn(name.clone()));
        }
        let ty = check_type(e, Some(&schema))?;
        let col = Ty::from(schema.columns[i].ty);
        if ty != col && ty != Ty::Null {
            return Err(ExecError::TypeMismatch(format!(
                "cannot assign {ty:?} to column {name:?} of type {col:?}"
            )));
        }
        targets.push((i, e));
    }
    let victims = matching_rows(txn, table, &schema, filter)?;
    for old in &victims {
        let mut new = old.clone();
        for (i, e) in &targets {
            new[*i] = eval(e, Some((&schema, old)))?;
        }
        txn.write(table, &new)?;
    }
    Ok(QueryResult {
        rows_affected: victims.len(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::open(dir.path()).unwrap();
        (dir, e)
    }

    fn ints(r: &QueryResult, col: usize) -> Vec<Value> {
        r.rows.iter().map(|row| row[col].clone()).collect()
    }

    fn setup(s: &mut Session<'_>) {
        s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT)")
            .unwrap();
        s.execute("INSERT INTO t VALUES (1, 10, 'a'), (2, 20, 'b'), (3, NULL, 'c')")
            .unwrap();
    }

    #[test]
    fn create_insert_select_round_trip() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        let r = s.execute("SELECT * FROM t").unwrap();
        assert_eq!(r.columns, vec!["id", "n", "s"]);
        assert_eq!(r.rows.len(), 3);
        assert_eq!(r.rows[2][1], Value::Null);
    }

    #[test]
    fn where_filters_and_null_never_matches_a_comparison() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        let r = s.execute("SELECT id FROM t WHERE n > 5").unwrap();
        assert_eq!(ints(&r, 0), vec![Value::Integer(1), Value::Integer(2)]);
        let r = s.execute("SELECT id FROM t WHERE n IS NULL").unwrap();
        assert_eq!(ints(&r, 0), vec![Value::Integer(3)]);
        let r = s.execute("SELECT id FROM t WHERE NOT n > 5").unwrap();
        assert!(r.rows.is_empty(), "NOT NULL is NULL, so row 3 is dropped");
    }

    #[test]
    fn order_by_limit_and_nulls_first() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        let r = s.execute("SELECT id FROM t ORDER BY n").unwrap();
        assert_eq!(
            ints(&r, 0),
            vec![Value::Integer(3), Value::Integer(1), Value::Integer(2)]
        );
        let r = s
            .execute("SELECT id FROM t ORDER BY n DESC LIMIT 1")
            .unwrap();
        assert_eq!(ints(&r, 0), vec![Value::Integer(2)]);
    }

    #[test]
    fn update_evaluates_against_the_old_row_and_reports_count() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        let r = s
            .execute("UPDATE t SET n = n + 1 WHERE n IS NOT NULL")
            .unwrap();
        assert_eq!(r.rows_affected, 2);
        let r = s.execute("SELECT n FROM t ORDER BY id").unwrap();
        assert_eq!(
            ints(&r, 0),
            vec![Value::Integer(11), Value::Integer(21), Value::Null]
        );
    }

    #[test]
    fn delete_removes_matching_rows() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        assert_eq!(
            s.execute("DELETE FROM t WHERE id >= 2")
                .unwrap()
                .rows_affected,
            2
        );
        assert_eq!(s.execute("SELECT * FROM t").unwrap().rows.len(), 1);
    }

    #[test]
    fn constraints_and_errors() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        assert!(matches!(
            s.execute("INSERT INTO t VALUES (1, 0, 'dup')"),
            Err(ExecError::DuplicatePrimaryKey(_))
        ));
        assert!(matches!(
            s.execute("INSERT INTO t (n) VALUES (5)"),
            Err(ExecError::NullPrimaryKey)
        ));
        assert!(matches!(
            s.execute("INSERT INTO t VALUES (9, 'x', 'y')"),
            Err(ExecError::Db(_))
        ));
        assert!(matches!(
            s.execute("INSERT INTO t VALUES (9, 1)"),
            Err(ExecError::ValueCount { .. })
        ));
        assert!(matches!(
            s.execute("UPDATE t SET id = 5"),
            Err(ExecError::UpdatePrimaryKey(_))
        ));
        assert!(matches!(
            s.execute("SELECT nope FROM t"),
            Err(ExecError::UnknownColumn(_))
        ));
        assert!(matches!(
            s.execute("SELECT * FROM ghosts"),
            Err(ExecError::Db(DbError::UnknownTable(_)))
        ));
        assert!(matches!(
            s.execute("CREATE TABLE u (a INTEGER)"),
            Err(ExecError::PrimaryKeyCount)
        ));
        assert!(matches!(
            s.execute("SELECT * FROM t WHERE n"),
            Err(ExecError::TypeMismatch(_))
        ));
    }

    #[test]
    fn failed_autocommit_statement_leaves_no_partial_rows() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        assert!(s
            .execute("INSERT INTO t VALUES (10, 1, 'x'), (11, 2, 'y'), (1, 3, 'dup')")
            .is_err());
        assert_eq!(s.execute("SELECT * FROM t").unwrap().rows.len(), 3);
    }

    #[test]
    fn explicit_transaction_commit_and_rollback() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        s.execute("BEGIN").unwrap();
        s.execute("DELETE FROM t").unwrap();
        assert!(s.execute("SELECT * FROM t").unwrap().rows.is_empty());
        s.execute("ROLLBACK").unwrap();
        assert_eq!(s.execute("SELECT * FROM t").unwrap().rows.len(), 3);
        s.execute("BEGIN").unwrap();
        s.execute("DELETE FROM t WHERE id = 1").unwrap();
        s.execute("COMMIT").unwrap();
        assert_eq!(s.execute("SELECT * FROM t").unwrap().rows.len(), 2);
    }

    #[test]
    fn an_error_poisons_the_transaction_until_rollback() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        s.execute("BEGIN").unwrap();
        s.execute("DELETE FROM t WHERE id = 1").unwrap();
        assert!(s.execute("INSERT INTO t VALUES (2, 0, 'dup')").is_err());
        assert!(matches!(
            s.execute("SELECT * FROM t"),
            Err(ExecError::TransactionAborted)
        ));
        assert!(matches!(
            s.execute("COMMIT"),
            Err(ExecError::TransactionAborted)
        ));
        assert!(!s.in_transaction());
        assert_eq!(
            s.execute("SELECT * FROM t").unwrap().rows.len(),
            3,
            "the aborted transaction's delete must not have applied"
        );
    }

    #[test]
    fn transaction_control_misuse() {
        let (_d, e) = engine();
        let mut s = e.session();
        assert!(matches!(
            s.execute("COMMIT"),
            Err(ExecError::NoActiveTransaction)
        ));
        assert!(matches!(
            s.execute("ROLLBACK"),
            Err(ExecError::NoActiveTransaction)
        ));
        s.execute("BEGIN").unwrap();
        assert!(matches!(
            s.execute("BEGIN"),
            Err(ExecError::AlreadyInTransaction)
        ));
        assert!(matches!(
            s.execute("CREATE TABLE u (a INTEGER PRIMARY KEY)"),
            Err(ExecError::DdlInTransaction)
        ));
    }

    #[test]
    fn two_sessions_conflict_first_committer_wins() {
        let (_d, e) = engine();
        let mut a = e.session();
        let mut b = e.session();
        setup(&mut a);
        a.execute("BEGIN").unwrap();
        b.execute("BEGIN").unwrap();
        a.execute("UPDATE t SET s = 'A' WHERE id = 1").unwrap();
        b.execute("UPDATE t SET s = 'B' WHERE id = 1").unwrap();
        a.execute("COMMIT").unwrap();
        assert!(matches!(
            b.execute("COMMIT"),
            Err(ExecError::Db(DbError::Conflict { .. }))
        ));
        let r = a.execute("SELECT s FROM t WHERE id = 1").unwrap();
        assert_eq!(r.rows[0][0], Value::Text("A".into()));
    }

    #[test]
    fn snapshot_isolation_hides_later_commits_from_an_open_transaction() {
        let (_d, e) = engine();
        let mut a = e.session();
        let mut b = e.session();
        setup(&mut a);
        b.execute("BEGIN").unwrap();
        a.execute("INSERT INTO t VALUES (4, 40, 'd')").unwrap();
        assert_eq!(b.execute("SELECT * FROM t").unwrap().rows.len(), 3);
        b.execute("COMMIT").unwrap();
        assert_eq!(b.execute("SELECT * FROM t").unwrap().rows.len(), 4);
    }

    #[test]
    fn access_path_picks_point_lookup_only_when_safe() {
        let (_d, e) = engine();
        let mut s = e.session();
        setup(&mut s);
        let schema = schema_of(e.database(), "t").unwrap();
        let path = |w: &str| {
            let Statement::Select { filter, .. } =
                sql::parse(&format!("SELECT * FROM t WHERE {w}")).unwrap()
            else {
                panic!()
            };
            access_path(&schema, filter.as_ref())
        };
        assert_eq!(path("id = 2"), AccessPath::PointLookup(Value::Integer(2)));
        assert_eq!(path("2 = id"), AccessPath::PointLookup(Value::Integer(2)));
        assert_eq!(
            path("n > 1 AND id = 1 + 1"),
            AccessPath::PointLookup(Value::Integer(2))
        );
        assert_eq!(path("id = 2 OR n = 1"), AccessPath::FullScan);
        assert_eq!(path("id > 2"), AccessPath::FullScan);
        assert_eq!(path("id = 'x'"), AccessPath::FullScan);
        assert_eq!(path("id = n"), AccessPath::FullScan);
        assert_eq!(path("id = NULL"), AccessPath::FullScan);
    }

    #[test]
    fn data_survives_a_real_close_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let e = Engine::open(dir.path()).unwrap();
            setup(&mut e.session());
        }
        let e = Engine::open(dir.path()).unwrap();
        assert_eq!(
            e.session().execute("SELECT * FROM t").unwrap().rows.len(),
            3
        );
    }

    #[test]
    fn scripts_run_in_order_and_stop_at_the_first_error() {
        let (_d, e) = engine();
        let mut s = e.session();
        let r = s.execute_script(
            "CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t VALUES (1); SELECT * FROM t",
        );
        assert_eq!(r.unwrap()[2].rows.len(), 1);
        assert!(s
            .execute_script(
                "INSERT INTO t VALUES (2); INSERT INTO t VALUES (2); INSERT INTO t VALUES (3)"
            )
            .is_err());
        assert_eq!(s.execute("SELECT * FROM t").unwrap().rows.len(), 2);
    }

    #[test]
    fn type_errors_do_not_depend_on_whether_rows_exist() {
        let (_d, e) = engine();
        let mut s = e.session();
        s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT)")
            .unwrap();
        for q in [
            "SELECT * FROM t WHERE n",
            "SELECT * FROM t WHERE s = 1",
            "DELETE FROM t WHERE n + 'a' > 1",
            "UPDATE t SET n = 'x'",
        ] {
            assert!(
                matches!(s.execute(q), Err(ExecError::TypeMismatch(_))),
                "{q} on an empty table"
            );
        }
        assert!(s.execute("UPDATE t SET n = NULL").is_ok());
    }
}
