//! Ticket 001's other property test: an arbitrary sequence of
//! `CREATE TABLE` + row put/delete operations, replayed through a fresh
//! `sietch::Store::open` (a real close and reopen — exercising sietch's
//! own crash-safety guarantees transitively, not just this crate's own
//! in-memory bookkeeping), reads back identically.

use std::collections::HashMap;

use catalog::Catalog;
use proptest::prelude::*;
use row::{encode_row, row_key, ColumnDef, ColumnType, Value};

#[derive(Debug, Clone)]
enum Op {
    CreateTable { name: String },
    Put { table: usize, pk: i64, text: String },
    Delete { table: usize, pk: i64 },
}

fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,8}"
}

fn arb_op(existing_tables: usize) -> impl Strategy<Value = Op> {
    if existing_tables == 0 {
        arb_ident()
            .prop_map(|name| Op::CreateTable { name })
            .boxed()
    } else {
        prop_oneof![
            2 => arb_ident().prop_map(|name| Op::CreateTable { name }),
            4 => (0..existing_tables, any::<i64>(), ".*").prop_map(|(table, pk, text)| Op::Put {
                table,
                pk,
                text,
            }),
            2 => (0..existing_tables, any::<i64>()).prop_map(|(table, pk)| Op::Delete { table, pk }),
        ]
        .boxed()
    }
}

fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
    // Grow the op list step by step so later ops can reference
    // already-created tables by index.
    (1usize..30).prop_flat_map(|n| {
        let mut strat = Just(Vec::<Op>::new()).boxed();
        for _ in 0..n {
            strat = strat
                .prop_flat_map(|ops: Vec<Op>| {
                    let tables = ops
                        .iter()
                        .filter(|o| matches!(o, Op::CreateTable { .. }))
                        .count();
                    arb_op(tables).prop_map(move |op| {
                        let mut next = ops.clone();
                        next.push(op);
                        next
                    })
                })
                .boxed();
        }
        strat
    })
}

fn schema_for(_name: &str) -> (Vec<ColumnDef>, usize) {
    (
        vec![
            ColumnDef {
                name: "id".into(),
                ty: ColumnType::Integer,
                nullable: false,
            },
            ColumnDef {
                name: "text".into(),
                ty: ColumnType::Text,
                nullable: false,
            },
        ],
        0,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn state_survives_a_real_close_and_reopen(ops in arb_ops()) {
        let dir = tempfile::tempdir().unwrap();

        // Reference model: table name -> (table_id assigned, pk -> text).
        let mut tables: Vec<(String, u32)> = Vec::new();
        let mut data: HashMap<(u32, i64), String> = HashMap::new();

        {
            let mut catalog = Catalog::open(dir.path()).unwrap();
            for op in &ops {
                match op {
                    Op::CreateTable { name } => {
                        // Duplicate names are possible from the generator;
                        // skip (matching what a real caller would do after
                        // seeing `TableAlreadyExists`) rather than treating
                        // it as a test failure.
                        if catalog.get_table(name).unwrap().is_some() {
                            continue;
                        }
                        let (cols, pk_idx) = schema_for(name);
                        let handle = catalog.create_table(name.clone(), cols, pk_idx).unwrap();
                        tables.push((name.clone(), handle.table_id));
                    }
                    Op::Put { table, pk, text } => {
                        let Some((name, table_id)) = tables.get(*table) else { continue };
                        let schema = catalog.get_table(name).unwrap().unwrap().schema;
                        let row = vec![Value::Integer(*pk), Value::Text(text.clone())];
                        let encoded = encode_row(&schema, &row).unwrap();
                        let key = row_key(*table_id, &Value::Integer(*pk), ColumnType::Integer).unwrap();
                        catalog.store_mut().put(key, encoded).unwrap();
                        data.insert((*table_id, *pk), text.clone());
                    }
                    Op::Delete { table, pk } => {
                        let Some((_, table_id)) = tables.get(*table) else { continue };
                        let key = row_key(*table_id, &Value::Integer(*pk), ColumnType::Integer).unwrap();
                        catalog.store_mut().delete(key).unwrap();
                        data.remove(&(*table_id, *pk));
                    }
                }
            }
        } // Catalog (and its Store) dropped here — a real close.

        // Reopen fresh and verify every table and every live row reads
        // back identically.
        let catalog = Catalog::open(dir.path()).unwrap();
        for (name, table_id) in &tables {
            let handle = catalog.get_table(name).unwrap().unwrap();
            prop_assert_eq!(handle.table_id, *table_id);
        }
        for ((table_id, pk), expected_text) in &data {
            let (_, schema) = tables
                .iter()
                .find_map(|(n, id)| (id == table_id).then(|| (n.clone(), catalog.get_table(n).unwrap().unwrap().schema)))
                .unwrap();
            let key = row_key(*table_id, &Value::Integer(*pk), ColumnType::Integer).unwrap();
            let stored = catalog.store().get(&key).unwrap();
            let decoded = row::decode_row(&schema, &stored).unwrap();
            prop_assert_eq!(&decoded[1], &Value::Text(expected_text.clone()));
        }
        // And nothing that was deleted (or never written) is still there.
        for (name, table_id) in &tables {
            let live_count = catalog.store().scan(&row::row_key_prefix(*table_id)).len();
            let expected_count = data.keys().filter(|(t, _)| t == table_id).count();
            prop_assert_eq!(live_count, expected_count, "table {}", name);
        }
    }
}
