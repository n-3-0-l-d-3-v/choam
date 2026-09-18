//! Serializes catalog entries for storage as value blobs: a
//! `(table_id, Schema)` pair for one table's entry, and a plain list of
//! table names for the index (ticket 002 — see `catalog.rs`). Both are
//! deliberately hand-rolled and independent of `row::encode_row`/
//! `decode_row` (which encode a table's *data* rows against a schema
//! already known) — these entries either *are* the schema or describe
//! what schemas exist, so nothing here can depend on already having one.

use row::{ColumnDef, ColumnType, Schema, SchemaError};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogCodecError {
    #[error("catalog entry data truncated")]
    Truncated,
    #[error("catalog entry contains invalid UTF-8 in a name")]
    InvalidUtf8,
    #[error("unknown column type tag {0}")]
    UnknownTypeTag(u8),
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

fn type_tag(ty: ColumnType) -> u8 {
    match ty {
        ColumnType::Integer => 0,
        ColumnType::Text => 1,
        ColumnType::Bytes => 2,
        ColumnType::Boolean => 3,
    }
}

fn type_from_tag(tag: u8) -> Result<ColumnType, CatalogCodecError> {
    match tag {
        0 => Ok(ColumnType::Integer),
        1 => Ok(ColumnType::Text),
        2 => Ok(ColumnType::Bytes),
        3 => Ok(ColumnType::Boolean),
        other => Err(CatalogCodecError::UnknownTypeTag(other)),
    }
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn read_string(data: &[u8]) -> Result<(&str, &[u8]), CatalogCodecError> {
    if data.len() < 2 {
        return Err(CatalogCodecError::Truncated);
    }
    let len = u16::from_le_bytes(data[0..2].try_into().unwrap()) as usize;
    if data.len() < 2 + len {
        return Err(CatalogCodecError::Truncated);
    }
    let s = std::str::from_utf8(&data[2..2 + len]).map_err(|_| CatalogCodecError::InvalidUtf8)?;
    Ok((s, &data[2 + len..]))
}

pub fn encode_table_entry(table_id: u32, schema: &Schema) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&table_id.to_le_bytes());
    write_string(&mut out, &schema.table_name);
    out.extend_from_slice(&(schema.columns.len() as u16).to_le_bytes());
    out.extend_from_slice(&(schema.primary_key as u16).to_le_bytes());
    for col in &schema.columns {
        write_string(&mut out, &col.name);
        out.push(type_tag(col.ty));
        out.push(u8::from(col.nullable));
    }
    out
}

pub fn decode_table_entry(data: &[u8]) -> Result<(u32, Schema), CatalogCodecError> {
    if data.len() < 4 {
        return Err(CatalogCodecError::Truncated);
    }
    let table_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let (table_name, rest) = read_string(&data[4..])?;
    if rest.len() < 4 {
        return Err(CatalogCodecError::Truncated);
    }
    let ncols = u16::from_le_bytes(rest[0..2].try_into().unwrap()) as usize;
    let primary_key = u16::from_le_bytes(rest[2..4].try_into().unwrap()) as usize;
    let mut rest = &rest[4..];
    let mut columns = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        let (name, after_name) = read_string(rest)?;
        if after_name.len() < 2 {
            return Err(CatalogCodecError::Truncated);
        }
        let ty = type_from_tag(after_name[0])?;
        let nullable = after_name[1] != 0;
        columns.push(ColumnDef {
            name: name.to_string(),
            ty,
            nullable,
        });
        rest = &after_name[2..];
    }
    let schema = Schema::new(table_name, columns, primary_key)?;
    Ok((table_id, schema))
}

/// Encodes the catalog's table-name index (ticket 002): a plain count-
/// prefixed list of names, in whatever order they were given.
pub fn encode_table_index(names: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for name in names {
        write_string(&mut out, name);
    }
    out
}

pub fn decode_table_index(data: &[u8]) -> Result<Vec<String>, CatalogCodecError> {
    if data.len() < 4 {
        return Err(CatalogCodecError::Truncated);
    }
    let count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut rest = &data[4..];
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, after) = read_string(rest)?;
        names.push(name.to_string());
        rest = after;
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schema() -> Schema {
        Schema::new(
            "users",
            vec![
                ColumnDef {
                    name: "id".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
                ColumnDef {
                    name: "email".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
            ],
            0,
        )
        .unwrap()
    }

    #[test]
    fn table_entry_round_trips() {
        let schema = sample_schema();
        let encoded = encode_table_entry(7, &schema);
        let (table_id, decoded) = decode_table_entry(&encoded).unwrap();
        assert_eq!(table_id, 7);
        assert_eq!(decoded, schema);
    }

    #[test]
    fn decode_rejects_truncated_input() {
        let encoded = encode_table_entry(1, &sample_schema());
        for cut in 0..encoded.len() {
            assert!(decode_table_entry(&encoded[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn decode_rejects_an_unknown_type_tag() {
        let mut encoded = encode_table_entry(1, &sample_schema());
        // Corrupt the first column's type tag byte. Layout: id(4) +
        // name_len(2)+name(5="users") + ncols(2) + pk(2) + col name_len(2)+name(2="id") + type(1).
        let type_tag_offset = 4 + 2 + 5 + 2 + 2 + 2 + 2;
        encoded[type_tag_offset] = 255;
        assert_eq!(
            decode_table_entry(&encoded),
            Err(CatalogCodecError::UnknownTypeTag(255))
        );
    }

    #[test]
    fn table_index_round_trips_including_empty() {
        for names in [
            vec![],
            vec!["a".to_string()],
            vec!["users".to_string(), "orders".to_string(), "".to_string()],
        ] {
            let encoded = encode_table_index(&names);
            assert_eq!(decode_table_index(&encoded).unwrap(), names);
        }
    }

    #[test]
    fn table_index_decode_rejects_truncated_input() {
        let encoded = encode_table_index(&["users".to_string(), "orders".to_string()]);
        for cut in 0..encoded.len() {
            assert!(decode_table_index(&encoded[..cut]).is_err(), "cut at {cut}");
        }
    }
}
