//! Typed column values and their two distinct encodings: an
//! **order-preserving** one (used only for primary-key components of a
//! `sietch` key — see `key.rs`) and an ordinary one (used for a row's
//! stored value blob, where byte order doesn't matter, only that it
//! round-trips exactly).
//!
//! Column types supported in this ticket: integers, text, bytes,
//! booleans, and nullability. No composite/multi-column primary keys
//! yet (see `docs/design/decisions/ADR-001-row-encoding-and-catalog.md`
//! for why that's a stated, deliberate limitation rather than an
//! oversight).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnType {
    Integer,
    Text,
    Bytes,
    Boolean,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i64),
    Text(String),
    Bytes(Vec<u8>),
    Boolean(bool),
    Null,
}

impl Value {
    pub fn column_type(&self) -> Option<ColumnType> {
        match self {
            Value::Integer(_) => Some(ColumnType::Integer),
            Value::Text(_) => Some(ColumnType::Text),
            Value::Bytes(_) => Some(ColumnType::Bytes),
            Value::Boolean(_) => Some(ColumnType::Boolean),
            Value::Null => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValueError {
    #[error("expected a {expected:?} value, found {found}")]
    TypeMismatch {
        expected: ColumnType,
        found: &'static str,
    },
    #[error("value data truncated while decoding a {0:?}")]
    Truncated(ColumnType),
    #[error("text column contains invalid UTF-8")]
    InvalidUtf8,
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Integer(_) => "Integer",
        Value::Text(_) => "Text",
        Value::Bytes(_) => "Bytes",
        Value::Boolean(_) => "Boolean",
        Value::Null => "Null",
    }
}

/// Encodes one non-null value into `out`, per its declared type — used
/// both for a row's stored value blob (`row.rs`) and, for whichever
/// column is the primary key, as the input to `key::encode_key_component`
/// (which then applies order-preserving transformations on top of this).
/// Variable-length types are length-prefixed (u32 LE) so multiple encoded
/// values can be concatenated and later split back apart unambiguously.
pub fn encode_value(
    value: &Value,
    expected: ColumnType,
    out: &mut Vec<u8>,
) -> Result<(), ValueError> {
    match (value, expected) {
        (Value::Integer(v), ColumnType::Integer) => {
            out.extend_from_slice(&v.to_be_bytes());
            Ok(())
        }
        (Value::Text(s), ColumnType::Text) => {
            let bytes = s.as_bytes();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
            Ok(())
        }
        (Value::Bytes(b), ColumnType::Bytes) => {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
            Ok(())
        }
        (Value::Boolean(b), ColumnType::Boolean) => {
            out.push(u8::from(*b));
            Ok(())
        }
        (other, expected) => Err(ValueError::TypeMismatch {
            expected,
            found: kind_name(other),
        }),
    }
}

/// Decodes one non-null value of `expected` type from the front of
/// `data`, returning it plus the remaining bytes.
pub fn decode_value(data: &[u8], expected: ColumnType) -> Result<(Value, &[u8]), ValueError> {
    match expected {
        ColumnType::Integer => {
            if data.len() < 8 {
                return Err(ValueError::Truncated(ColumnType::Integer));
            }
            let v = i64::from_be_bytes(data[0..8].try_into().unwrap());
            Ok((Value::Integer(v), &data[8..]))
        }
        ColumnType::Boolean => {
            if data.is_empty() {
                return Err(ValueError::Truncated(ColumnType::Boolean));
            }
            Ok((Value::Boolean(data[0] != 0), &data[1..]))
        }
        ColumnType::Text => {
            let (bytes, rest) = decode_length_prefixed(data, ColumnType::Text)?;
            let s = String::from_utf8(bytes.to_vec()).map_err(|_| ValueError::InvalidUtf8)?;
            Ok((Value::Text(s), rest))
        }
        ColumnType::Bytes => {
            let (bytes, rest) = decode_length_prefixed(data, ColumnType::Bytes)?;
            Ok((Value::Bytes(bytes.to_vec()), rest))
        }
    }
}

fn decode_length_prefixed(data: &[u8], ty: ColumnType) -> Result<(&[u8], &[u8]), ValueError> {
    if data.len() < 4 {
        return Err(ValueError::Truncated(ty));
    }
    let len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    if data.len() < 4 + len {
        return Err(ValueError::Truncated(ty));
    }
    Ok((&data[4..4 + len], &data[4 + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_round_trips() {
        let cases = [
            (Value::Integer(-42), ColumnType::Integer),
            (Value::Integer(i64::MIN), ColumnType::Integer),
            (Value::Integer(i64::MAX), ColumnType::Integer),
            (Value::Text("hello".into()), ColumnType::Text),
            (Value::Text(String::new()), ColumnType::Text),
            (Value::Bytes(vec![1, 2, 3]), ColumnType::Bytes),
            (Value::Bytes(vec![]), ColumnType::Bytes),
            (Value::Boolean(true), ColumnType::Boolean),
            (Value::Boolean(false), ColumnType::Boolean),
        ];
        for (v, ty) in cases {
            let mut buf = Vec::new();
            encode_value(&v, ty, &mut buf).unwrap();
            let (decoded, rest) = decode_value(&buf, ty).unwrap();
            assert_eq!(decoded, v);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn concatenated_values_split_back_apart_unambiguously() {
        let mut buf = Vec::new();
        encode_value(&Value::Text("ab".into()), ColumnType::Text, &mut buf).unwrap();
        encode_value(&Value::Integer(7), ColumnType::Integer, &mut buf).unwrap();
        encode_value(&Value::Bytes(vec![9, 9]), ColumnType::Bytes, &mut buf).unwrap();

        let (v1, rest) = decode_value(&buf, ColumnType::Text).unwrap();
        let (v2, rest) = decode_value(rest, ColumnType::Integer).unwrap();
        let (v3, rest) = decode_value(rest, ColumnType::Bytes).unwrap();
        assert_eq!(v1, Value::Text("ab".into()));
        assert_eq!(v2, Value::Integer(7));
        assert_eq!(v3, Value::Bytes(vec![9, 9]));
        assert!(rest.is_empty());
    }

    #[test]
    fn encode_rejects_a_type_mismatch() {
        let mut buf = Vec::new();
        assert_eq!(
            encode_value(&Value::Integer(1), ColumnType::Text, &mut buf),
            Err(ValueError::TypeMismatch {
                expected: ColumnType::Text,
                found: "Integer",
            })
        );
    }

    #[test]
    fn decode_rejects_truncated_input_for_every_type() {
        for ty in [
            ColumnType::Integer,
            ColumnType::Boolean,
            ColumnType::Text,
            ColumnType::Bytes,
        ] {
            assert!(decode_value(&[], ty).is_err());
        }
    }

    #[test]
    fn decode_rejects_invalid_utf8() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&[0xFF, 0xFE]);
        assert_eq!(
            decode_value(&buf, ColumnType::Text),
            Err(ValueError::InvalidUtf8)
        );
    }
}
