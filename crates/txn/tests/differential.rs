//! Differential test (ticket 002): an arbitrary sequence of single-
//! statement transactions (put/get/delete), each committed immediately,
//! replayed in the same order against `txn::Database` and against a
//! plain `HashMap` reference model. After every operation the two must
//! agree on that key's value — and, at the end, agree on the whole
//! table's contents.

use std::collections::HashMap;

use proptest::prelude::*;
use row::{ColumnDef, ColumnType, Value};
use txn::Database;

#[derive(Debug, Clone)]
enum Op {
    Put(i64, String),
    Delete(i64),
    Get(i64),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0i64..8, ".*").prop_map(|(k, v)| Op::Put(k, v)),
        2 => (0i64..8).prop_map(Op::Delete),
        2 => (0i64..8).prop_map(Op::Get),
    ]
}

fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(arb_op(), 0..60)
}

fn cols() -> Vec<ColumnDef> {
    vec![
        ColumnDef {
            name: "id".into(),
            ty: ColumnType::Integer,
            nullable: false,
        },
        ColumnDef {
            name: "value".into(),
            ty: ColumnType::Text,
            nullable: false,
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn matches_a_plain_hashmap_reference_model(ops in arb_ops()) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).unwrap();
        db.create_table("kv", cols(), 0).unwrap();

        let mut model: HashMap<i64, String> = HashMap::new();

        for op in &ops {
            match op {
                Op::Put(k, v) => {
                    let mut txn = db.begin();
                    txn.write("kv", &[Value::Integer(*k), Value::Text(v.clone())]).unwrap();
                    txn.commit().unwrap();
                    model.insert(*k, v.clone());
                }
                Op::Delete(k) => {
                    let mut txn = db.begin();
                    txn.delete("kv", &Value::Integer(*k)).unwrap();
                    txn.commit().unwrap();
                    model.remove(k);
                }
                Op::Get(k) => {
                    let txn = db.begin();
                    let actual = txn.read("kv", &Value::Integer(*k)).unwrap()
                        .map(|row| match &row[1] {
                            Value::Text(s) => s.clone(),
                            other => panic!("expected Text, got {other:?}"),
                        });
                    prop_assert_eq!(&actual, &model.get(k).cloned(), "mismatch reading key {}", k);
                }
            }
        }

        // Final full-table comparison, independent of the per-op checks.
        for k in 0i64..8 {
            let txn = db.begin();
            let actual = txn.read("kv", &Value::Integer(k)).unwrap()
                .map(|row| match &row[1] {
                    Value::Text(s) => s.clone(),
                    other => panic!("expected Text, got {other:?}"),
                });
            prop_assert_eq!(&actual, &model.get(&k).cloned(), "final mismatch on key {}", k);
        }
    }
}
