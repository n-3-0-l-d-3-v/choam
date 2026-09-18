//! Ticket 001's property tests: arbitrary values round-trip exactly
//! through both encodings, and distinct `(table_id, primary_key)` pairs
//! never produce the same `sietch` key.

use proptest::prelude::*;
use row::{
    decode_row, decode_value, encode_row, encode_value, row_key, ColumnDef, ColumnType, Schema,
    Value,
};

fn arb_column_type() -> impl Strategy<Value = ColumnType> {
    prop_oneof![
        Just(ColumnType::Integer),
        Just(ColumnType::Text),
        Just(ColumnType::Bytes),
        Just(ColumnType::Boolean),
    ]
}

fn arb_value_of(ty: ColumnType) -> impl Strategy<Value = Value> {
    match ty {
        ColumnType::Integer => any::<i64>().prop_map(Value::Integer).boxed(),
        ColumnType::Text => ".*".prop_map(Value::Text).boxed(),
        ColumnType::Bytes => prop::collection::vec(any::<u8>(), 0..40)
            .prop_map(Value::Bytes)
            .boxed(),
        ColumnType::Boolean => any::<bool>().prop_map(Value::Boolean).boxed(),
    }
}

fn arb_nonnull_value() -> impl Strategy<Value = (ColumnType, Value)> {
    arb_column_type().prop_flat_map(|ty| arb_value_of(ty).prop_map(move |v| (ty, v)))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// A single value of arbitrary type round-trips through the ordinary
    /// (non-order-preserving) value codec.
    #[test]
    fn arbitrary_value_round_trips_through_the_value_codec((ty, value) in arb_nonnull_value()) {
        let mut buf = Vec::new();
        encode_value(&value, ty, &mut buf).unwrap();
        let (decoded, rest) = decode_value(&buf, ty).unwrap();
        prop_assert_eq!(decoded, value);
        prop_assert!(rest.is_empty());
    }

    /// An arbitrary full row (built from a schema whose column types are
    /// also arbitrary) round-trips through `encode_row`/`decode_row`,
    /// nulls included wherever a column happens to be nullable.
    #[test]
    fn arbitrary_row_round_trips(
        specs in prop::collection::vec((arb_column_type(), any::<bool>()), 1..8),
        seed in any::<u64>(),
    ) {
        let columns: Vec<ColumnDef> = specs
            .iter()
            .enumerate()
            .map(|(i, &(ty, nullable))| ColumnDef {
                name: format!("c{i}"),
                ty,
                // the primary key (index 0) can never be nullable
                nullable: nullable && i != 0,
            })
            .collect();
        let schema = Schema::new("t", columns, 0).unwrap();

        // Deterministic per-column values derived from `seed`, including
        // nulls for nullable columns roughly half the time.
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            state
        };
        let row: Vec<Value> = schema
            .columns
            .iter()
            .map(|col| {
                if col.nullable && next() % 2 == 0 {
                    Value::Null
                } else {
                    match col.ty {
                        ColumnType::Integer => Value::Integer(next() as i64),
                        ColumnType::Text => Value::Text(format!("v{}", next())),
                        ColumnType::Bytes => Value::Bytes(next().to_le_bytes().to_vec()),
                        ColumnType::Boolean => Value::Boolean(next() % 2 == 0),
                    }
                }
            })
            .collect();

        let encoded = encode_row(&schema, &row).unwrap();
        let decoded = decode_row(&schema, &encoded).unwrap();
        prop_assert_eq!(decoded, row);
    }

    /// Within one table (so, realistically, one fixed primary-key type —
    /// a table only ever declares one), distinct primary keys never
    /// produce the same row key, and equal ones always do.
    #[test]
    fn distinct_primary_keys_in_the_same_table_never_collide(
        table in any::<u32>(),
        (ty, pk_a, pk_b) in arb_column_type()
            .prop_flat_map(|ty| (Just(ty), arb_value_of(ty), arb_value_of(ty))),
    ) {
        let key_a = row_key(table, &pk_a, ty).unwrap();
        let key_b = row_key(table, &pk_b, ty).unwrap();
        if pk_a != pk_b {
            prop_assert_ne!(key_a, key_b);
        } else {
            prop_assert_eq!(key_a, key_b);
        }
    }

    /// Different tables never share a row key, regardless of primary-key
    /// type or value on either side — the fixed-width `table_id` prefix
    /// makes this true unconditionally.
    #[test]
    fn different_tables_never_collide_regardless_of_key_type_or_value(
        table_a in any::<u32>(),
        table_b in any::<u32>(),
        (ty_a, pk_a) in arb_nonnull_value(),
        (ty_b, pk_b) in arb_nonnull_value(),
    ) {
        prop_assume!(table_a != table_b);
        let key_a = row_key(table_a, &pk_a, ty_a);
        let key_b = row_key(table_b, &pk_b, ty_b);
        if let (Ok(key_a), Ok(key_b)) = (key_a, key_b) {
            prop_assert_ne!(key_a, key_b);
        }
    }
}
