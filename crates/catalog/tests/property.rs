//! An arbitrary sequence of `CREATE TABLE` calls, replayed through a
//! fresh `Catalog::open` (a real close and reopen — exercising
//! `sietch::TransactionalStore`'s own crash-safety guarantees
//! transitively, not just this crate's own in-memory bookkeeping), reads
//! back identically. Row-data put/delete/reopen coverage now lives in
//! `crates/txn`'s own property test (ticket 002) — this crate no longer
//! exposes a raw store to put/delete/scan row data through, since
//! `Catalog` moved onto `TransactionalStore`, which is shared with the
//! relational transaction layer rather than something this test should
//! poke directly.

use catalog::Catalog;
use proptest::prelude::*;
use row::{ColumnDef, ColumnType};

fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,8}"
}

fn arb_names() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(arb_ident(), 0..20)
}

fn schema_cols() -> Vec<ColumnDef> {
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
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn state_survives_a_real_close_and_reopen(names in arb_names()) {
        let dir = tempfile::tempdir().unwrap();

        // Reference model: every name actually created (duplicates from
        // the generator are skipped, matching what a real caller would do
        // after seeing `TableAlreadyExists`), with its assigned table id.
        let mut created: Vec<(String, u32)> = Vec::new();

        {
            let catalog = Catalog::open(dir.path()).unwrap();
            for name in &names {
                if catalog.get_table(name).unwrap().is_some() {
                    continue;
                }
                let handle = catalog.create_table(name.clone(), schema_cols(), 0).unwrap();
                created.push((name.clone(), handle.table_id));
            }
        } // Catalog (and its TransactionalStore) dropped here — a real close.

        let catalog = Catalog::open(dir.path()).unwrap();
        for (name, table_id) in &created {
            let handle = catalog.get_table(name).unwrap().unwrap();
            prop_assert_eq!(handle.table_id, *table_id);
        }
        let listed = catalog.list_tables().unwrap();
        prop_assert_eq!(listed.len(), created.len());
        for (name, table_id) in &created {
            prop_assert!(listed.iter().any(|h| &h.schema.table_name == name && h.table_id == *table_id));
        }
    }
}
