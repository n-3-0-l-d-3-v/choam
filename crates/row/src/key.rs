//! Encodes table/catalog identity into `sietch::Store`'s flat byte-key
//! space. Two disjoint namespaces, distinguished by their first byte, so
//! catalog entries and row data can never collide:
//!
//! ```text
//! [0x00]                        -- the catalog's own next-table-id counter
//! [0x00] ++ table_name bytes    -- one entry per table's schema
//! [0x01] ++ table_id (4 BE) ++ encoded primary key
//! ```
//!
//! A table name is validated non-empty (see `catalog`), so it can never
//! collide with the bare `[0x00]` counter key.
//!
//! The primary-key component is encoded **order-preserving**: two
//! primary keys compare the same way under Rust's own `Ord` as their
//! encoded bytes compare lexicographically. This isn't exercised by
//! `sietch::Store::scan` (a prefix match, not a range) in this ticket,
//! but every future range query this phase's SQL subset will want
//! depends on it, and it costs nothing to get right now — see ADR-001.

use crate::value::{ColumnType, Value, ValueError};

pub const CATALOG_NAMESPACE: u8 = 0x00;
pub const ROW_NAMESPACE: u8 = 0x01;

pub fn catalog_counter_key() -> Vec<u8> {
    vec![CATALOG_NAMESPACE]
}

/// `None` if `table_name` is empty (reserved for the counter key — see
/// module docs; `catalog` rejects empty table names before this is ever
/// called, so this is a defensive check, not the primary validation).
pub fn catalog_table_key(table_name: &str) -> Option<Vec<u8>> {
    if table_name.is_empty() {
        return None;
    }
    let mut key = vec![CATALOG_NAMESPACE];
    key.extend_from_slice(table_name.as_bytes());
    Some(key)
}

pub fn row_key_prefix(table_id: u32) -> Vec<u8> {
    let mut key = vec![ROW_NAMESPACE];
    key.extend_from_slice(&table_id.to_be_bytes());
    key
}

/// The full row key for `table_id`'s row with primary key `pk`. `pk`
/// must be a non-null value of `pk_type` (a table's declared primary-key
/// column type) — a `Null` primary key, or one of the wrong type, is
/// rejected, matching every real database's rule that a primary key can
/// never be null.
pub fn row_key(table_id: u32, pk: &Value, pk_type: ColumnType) -> Result<Vec<u8>, ValueError> {
    let mut key = row_key_prefix(table_id);
    encode_key_component(pk, pk_type, &mut key)?;
    Ok(key)
}

/// Appends `value`'s order-preserving encoding to `out`. Only ever
/// called with `value` as the *last* component of a key (there is no
/// composite/multi-column primary key in this ticket's scope — see
/// ADR-001), so variable-length encodings need no length prefix or
/// delimiter: whatever bytes remain simply *are* the whole component.
pub fn encode_key_component(
    value: &Value,
    expected: ColumnType,
    out: &mut Vec<u8>,
) -> Result<(), ValueError> {
    match (value, expected) {
        (Value::Integer(v), ColumnType::Integer) => {
            // Flip the sign bit so two's-complement negative numbers sort
            // before non-negative ones under plain unsigned byte
            // comparison — the standard order-preserving integer trick.
            let unsigned = (*v as u64) ^ (1u64 << 63);
            out.extend_from_slice(&unsigned.to_be_bytes());
            Ok(())
        }
        (Value::Text(s), ColumnType::Text) => {
            out.extend_from_slice(s.as_bytes());
            Ok(())
        }
        (Value::Bytes(b), ColumnType::Bytes) => {
            out.extend_from_slice(b);
            Ok(())
        }
        (Value::Boolean(b), ColumnType::Boolean) => {
            out.push(u8::from(*b));
            Ok(())
        }
        (Value::Null, _) => Err(ValueError::TypeMismatch {
            expected,
            found: "Null",
        }),
        (other, expected) => Err(ValueError::TypeMismatch {
            expected,
            found: match other {
                Value::Integer(_) => "Integer",
                Value::Text(_) => "Text",
                Value::Bytes(_) => "Bytes",
                Value::Boolean(_) => "Boolean",
                Value::Null => "Null",
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_row_namespaces_never_collide() {
        let counter = catalog_counter_key();
        let table = catalog_table_key("t").unwrap();
        let row = row_key(0, &Value::Integer(0), ColumnType::Integer).unwrap();
        assert_ne!(counter[0], ROW_NAMESPACE);
        assert_ne!(table[0], ROW_NAMESPACE);
        assert_eq!(row[0], ROW_NAMESPACE);
        assert_ne!(counter, table);
    }

    #[test]
    fn empty_table_name_is_rejected_reserved_for_the_counter() {
        assert_eq!(catalog_table_key(""), None);
    }

    #[test]
    fn integer_key_encoding_preserves_order() {
        let mut values: Vec<i64> = vec![i64::MIN, -1000, -1, 0, 1, 1000, i64::MAX];
        let mut encoded: Vec<Vec<u8>> = values
            .iter()
            .map(|v| {
                let mut out = Vec::new();
                encode_key_component(&Value::Integer(*v), ColumnType::Integer, &mut out).unwrap();
                out
            })
            .collect();
        let sorted_by_value = {
            let mut v = values.clone();
            v.sort();
            v
        };
        values.sort();
        assert_eq!(values, sorted_by_value);
        let mut encoded_sorted = encoded.clone();
        encoded_sorted.sort();
        assert_eq!(
            encoded, encoded_sorted,
            "encoded order must match integer order"
        );
        encoded.dedup();
        assert_eq!(
            encoded.len(),
            7,
            "every distinct integer must encode distinctly"
        );
    }

    #[test]
    fn text_key_encoding_preserves_lexicographic_order() {
        let words = ["", "a", "aa", "ab", "b", "ba"];
        let mut encoded: Vec<Vec<u8>> = words
            .iter()
            .map(|w| {
                let mut out = Vec::new();
                encode_key_component(&Value::Text((*w).into()), ColumnType::Text, &mut out)
                    .unwrap();
                out
            })
            .collect();
        let mut expected = encoded.clone();
        expected.sort();
        // Words are already listed in the order they should sort in.
        assert_eq!(encoded, expected);
        encoded.dedup();
        assert_eq!(encoded.len(), words.len());
    }

    #[test]
    fn a_null_primary_key_is_rejected() {
        assert!(row_key(0, &Value::Null, ColumnType::Integer).is_err());
    }

    #[test]
    fn a_wrong_typed_primary_key_is_rejected() {
        assert!(row_key(0, &Value::Text("x".into()), ColumnType::Integer).is_err());
    }

    #[test]
    fn different_tables_never_share_a_row_key_even_with_the_same_primary_key() {
        let a = row_key(1, &Value::Integer(5), ColumnType::Integer).unwrap();
        let b = row_key(2, &Value::Integer(5), ColumnType::Integer).unwrap();
        assert_ne!(a, b);
    }
}
