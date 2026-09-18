//! Encodes table/catalog identity into `sietch::Store`'s flat byte-key
//! space. Two disjoint namespaces, distinguished by their first byte, so
//! catalog entries and row data can never collide:
//!
//! ```text
//! [0x00, 0x00]                   -- the catalog's own next-table-id counter
//! [0x00, 0x01]                   -- the catalog's table-name index (ticket 002)
//! [0x00, 0x02] ++ table name     -- one entry per table's schema
//! [0x01] ++ table_id (4 BE) ++ encoded primary key
//! ```
//!
//! The catalog namespace got a second tag byte in ticket 002, when the
//! catalog moved onto `sietch::TransactionalStore` (which exposes no
//! `scan`, only `get`/`put`/`delete` — see
//! `docs/design/decisions/ADR-002-relational-transactions.md`) and
//! needed an explicit, transactionally-maintained index of table names
//! instead of a namespace scan to list them. A table name is still
//! validated non-empty, so it can never collide with either reserved
//! sub-key.
//!
//! The primary-key component is encoded **order-preserving**: two
//! primary keys compare the same way under Rust's own `Ord` as their
//! encoded bytes compare lexicographically. This isn't exercised by
//! `sietch::Store::scan` (a prefix match, not a range) in this ticket,
//! but every future range query this phase's SQL subset will want
//! depends on it, and it costs nothing to get right now — see ADR-001.
//! Because of this, and because a primary key is always the *only*
//! component after the key prefix, a row key's primary-key component can
//! also be decoded back (`decode_key_component`) — used by ticket 002 to
//! name the exact row a transaction conflict happened on.

use crate::value::{ColumnType, Value, ValueError};

pub const CATALOG_NAMESPACE: u8 = 0x00;
pub const ROW_NAMESPACE: u8 = 0x01;

const CATALOG_COUNTER_TAG: u8 = 0x00;
const CATALOG_INDEX_TAG: u8 = 0x01;
const CATALOG_TABLE_TAG: u8 = 0x02;

pub fn catalog_counter_key() -> Vec<u8> {
    vec![CATALOG_NAMESPACE, CATALOG_COUNTER_TAG]
}

/// The catalog's own list of table names (ticket 002) — a single key
/// whose value is a serialized `Vec<String>`, kept in sync with the
/// actual table entries transactionally (see `catalog::Catalog`), so
/// listing tables never needs a namespace scan.
pub fn catalog_index_key() -> Vec<u8> {
    vec![CATALOG_NAMESPACE, CATALOG_INDEX_TAG]
}

/// `None` if `table_name` is empty (reserved — see module docs;
/// `catalog` rejects empty table names before this is ever called, so
/// this is a defensive check, not the primary validation).
pub fn catalog_table_key(table_name: &str) -> Option<Vec<u8>> {
    if table_name.is_empty() {
        return None;
    }
    let mut key = vec![CATALOG_NAMESPACE, CATALOG_TABLE_TAG];
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

/// Splits an arbitrary key back into `(table_id, encoded primary key
/// bytes)`, if it's a well-formed row key at all (starts with
/// `ROW_NAMESPACE` and has at least the 4-byte table id). Used by ticket
/// 002 to identify which table a raw conflicting key (as reported by
/// `sietch::TxnError::Conflict`) belongs to.
pub fn split_row_key(key: &[u8]) -> Option<(u32, &[u8])> {
    if key.first() != Some(&ROW_NAMESPACE) || key.len() < 5 {
        return None;
    }
    let table_id = u32::from_be_bytes(key[1..5].try_into().unwrap());
    Some((table_id, &key[5..]))
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

/// The inverse of `encode_key_component`: decodes a primary-key
/// component of declared type `ty` from `bytes` (exactly the bytes
/// `split_row_key` returns for the primary-key part of a row key).
pub fn decode_key_component(bytes: &[u8], ty: ColumnType) -> Result<Value, ValueError> {
    match ty {
        ColumnType::Integer => {
            if bytes.len() != 8 {
                return Err(ValueError::Truncated(ColumnType::Integer));
            }
            let unsigned = u64::from_be_bytes(bytes.try_into().unwrap());
            let v = (unsigned ^ (1u64 << 63)) as i64;
            Ok(Value::Integer(v))
        }
        ColumnType::Boolean => {
            if bytes.len() != 1 {
                return Err(ValueError::Truncated(ColumnType::Boolean));
            }
            Ok(Value::Boolean(bytes[0] != 0))
        }
        ColumnType::Text => {
            let s = std::str::from_utf8(bytes).map_err(|_| ValueError::InvalidUtf8)?;
            Ok(Value::Text(s.to_string()))
        }
        ColumnType::Bytes => Ok(Value::Bytes(bytes.to_vec())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_row_namespaces_never_collide() {
        let counter = catalog_counter_key();
        let index = catalog_index_key();
        let table = catalog_table_key("t").unwrap();
        let row = row_key(0, &Value::Integer(0), ColumnType::Integer).unwrap();
        assert_ne!(counter[0], ROW_NAMESPACE);
        assert_ne!(index[0], ROW_NAMESPACE);
        assert_ne!(table[0], ROW_NAMESPACE);
        assert_eq!(row[0], ROW_NAMESPACE);
        assert_ne!(counter, table);
        assert_ne!(counter, index);
        assert_ne!(index, table);
    }

    #[test]
    fn empty_table_name_is_rejected_reserved_for_catalog_internals() {
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

    #[test]
    fn split_row_key_recovers_the_table_id_and_pk_bytes() {
        let key = row_key(42, &Value::Integer(-7), ColumnType::Integer).unwrap();
        let (table_id, pk_bytes) = split_row_key(&key).unwrap();
        assert_eq!(table_id, 42);
        assert_eq!(
            decode_key_component(pk_bytes, ColumnType::Integer).unwrap(),
            Value::Integer(-7)
        );
    }

    #[test]
    fn split_row_key_rejects_a_catalog_key() {
        assert_eq!(split_row_key(&catalog_counter_key()), None);
        assert_eq!(split_row_key(&catalog_table_key("t").unwrap()), None);
    }

    #[test]
    fn decode_key_component_round_trips_every_type() {
        for (ty, value) in [
            (ColumnType::Integer, Value::Integer(i64::MIN)),
            (ColumnType::Integer, Value::Integer(0)),
            (ColumnType::Integer, Value::Integer(i64::MAX)),
            (ColumnType::Text, Value::Text("hello".into())),
            (ColumnType::Text, Value::Text(String::new())),
            (ColumnType::Bytes, Value::Bytes(vec![1, 2, 3])),
            (ColumnType::Boolean, Value::Boolean(true)),
            (ColumnType::Boolean, Value::Boolean(false)),
        ] {
            let mut out = Vec::new();
            encode_key_component(&value, ty, &mut out).unwrap();
            assert_eq!(decode_key_component(&out, ty).unwrap(), value);
        }
    }
}
