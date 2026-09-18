//! A `Schema` (column definitions plus which one is the primary key) and
//! whole-row encode/decode built on `value`'s per-column codec, plus a
//! leading null-bitmap so nullable columns don't need a length prefix of
//! their own just to signal absence.

use crate::value::{decode_value, encode_value, ColumnType, Value, ValueError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub name: String,
    pub ty: ColumnType,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
    /// Index into `columns` of the primary-key column. Exactly one
    /// column, always non-nullable (enforced by `Schema::new`) — no
    /// composite primary keys in this ticket's scope, see ADR-001.
    pub primary_key: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaError {
    #[error("a table must have at least one column")]
    NoColumns,
    #[error("primary key index {0} is out of range for {1} columns")]
    PrimaryKeyOutOfRange(usize, usize),
    #[error("the primary key column ({0:?}) cannot be nullable")]
    NullablePrimaryKey(String),
    #[error("duplicate column name {0:?}")]
    DuplicateColumnName(String),
}

impl Schema {
    pub fn new(
        table_name: impl Into<String>,
        columns: Vec<ColumnDef>,
        primary_key: usize,
    ) -> Result<Schema, SchemaError> {
        if columns.is_empty() {
            return Err(SchemaError::NoColumns);
        }
        if primary_key >= columns.len() {
            return Err(SchemaError::PrimaryKeyOutOfRange(
                primary_key,
                columns.len(),
            ));
        }
        if columns[primary_key].nullable {
            return Err(SchemaError::NullablePrimaryKey(
                columns[primary_key].name.clone(),
            ));
        }
        for i in 0..columns.len() {
            for j in (i + 1)..columns.len() {
                if columns[i].name == columns[j].name {
                    return Err(SchemaError::DuplicateColumnName(columns[i].name.clone()));
                }
            }
        }
        Ok(Schema {
            table_name: table_name.into(),
            columns,
            primary_key,
        })
    }

    pub fn primary_key_type(&self) -> ColumnType {
        self.columns[self.primary_key].ty
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RowError {
    #[error("row has {found} values, schema declares {expected} columns")]
    ColumnCountMismatch { expected: usize, found: usize },
    #[error("column {0:?} is not nullable but was given a null value")]
    UnexpectedNull(String),
    #[error(transparent)]
    Value(#[from] ValueError),
    #[error("row data truncated")]
    Truncated,
}

fn bitmap_len(ncols: usize) -> usize {
    ncols.div_ceil(8)
}

/// Encodes a full row: a leading null-bitmap, then each non-null
/// column's value in schema order.
pub fn encode_row(schema: &Schema, values: &[Value]) -> Result<Vec<u8>, RowError> {
    if values.len() != schema.columns.len() {
        return Err(RowError::ColumnCountMismatch {
            expected: schema.columns.len(),
            found: values.len(),
        });
    }
    let mut bitmap = vec![0u8; bitmap_len(schema.columns.len())];
    let mut body = Vec::new();
    for (i, (value, col)) in values.iter().zip(&schema.columns).enumerate() {
        if value.is_null() {
            if !col.nullable {
                return Err(RowError::UnexpectedNull(col.name.clone()));
            }
            bitmap[i / 8] |= 1 << (i % 8);
        } else {
            encode_value(value, col.ty, &mut body)?;
        }
    }
    let mut out = bitmap;
    out.extend(body);
    Ok(out)
}

/// Decodes a full row previously produced by `encode_row` for the same
/// `schema`.
pub fn decode_row(schema: &Schema, data: &[u8]) -> Result<Vec<Value>, RowError> {
    let blen = bitmap_len(schema.columns.len());
    if data.len() < blen {
        return Err(RowError::Truncated);
    }
    let bitmap = &data[..blen];
    let mut rest = &data[blen..];
    let mut values = Vec::with_capacity(schema.columns.len());
    for (i, col) in schema.columns.iter().enumerate() {
        let is_null = bitmap[i / 8] & (1 << (i % 8)) != 0;
        if is_null {
            values.push(Value::Null);
        } else {
            let (v, remaining) = decode_value(rest, col.ty)?;
            values.push(v);
            rest = remaining;
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::new(
            "t",
            vec![
                ColumnDef {
                    name: "id".into(),
                    ty: ColumnType::Integer,
                    nullable: false,
                },
                ColumnDef {
                    name: "name".into(),
                    ty: ColumnType::Text,
                    nullable: true,
                },
                ColumnDef {
                    name: "active".into(),
                    ty: ColumnType::Boolean,
                    nullable: false,
                },
            ],
            0,
        )
        .unwrap()
    }

    #[test]
    fn a_row_with_no_nulls_round_trips() {
        let s = schema();
        let row = vec![
            Value::Integer(1),
            Value::Text("alice".into()),
            Value::Boolean(true),
        ];
        let encoded = encode_row(&s, &row).unwrap();
        assert_eq!(decode_row(&s, &encoded).unwrap(), row);
    }

    #[test]
    fn a_row_with_a_null_round_trips() {
        let s = schema();
        let row = vec![Value::Integer(2), Value::Null, Value::Boolean(false)];
        let encoded = encode_row(&s, &row).unwrap();
        assert_eq!(decode_row(&s, &encoded).unwrap(), row);
    }

    #[test]
    fn a_null_in_a_non_nullable_column_is_rejected() {
        let s = schema();
        let row = vec![Value::Null, Value::Null, Value::Boolean(false)];
        assert_eq!(
            encode_row(&s, &row),
            Err(RowError::UnexpectedNull("id".into()))
        );
    }

    #[test]
    fn wrong_column_count_is_rejected() {
        let s = schema();
        assert!(matches!(
            encode_row(&s, &[Value::Integer(1)]),
            Err(RowError::ColumnCountMismatch {
                expected: 3,
                found: 1
            })
        ));
    }

    #[test]
    fn schema_rejects_no_columns() {
        assert_eq!(Schema::new("t", vec![], 0), Err(SchemaError::NoColumns));
    }

    #[test]
    fn schema_rejects_nullable_primary_key() {
        let cols = vec![ColumnDef {
            name: "id".into(),
            ty: ColumnType::Integer,
            nullable: true,
        }];
        assert_eq!(
            Schema::new("t", cols, 0),
            Err(SchemaError::NullablePrimaryKey("id".into()))
        );
    }

    #[test]
    fn schema_rejects_duplicate_column_names() {
        let cols = vec![
            ColumnDef {
                name: "id".into(),
                ty: ColumnType::Integer,
                nullable: false,
            },
            ColumnDef {
                name: "id".into(),
                ty: ColumnType::Text,
                nullable: true,
            },
        ];
        assert_eq!(
            Schema::new("t", cols, 0),
            Err(SchemaError::DuplicateColumnName("id".into()))
        );
    }

    #[test]
    fn schema_rejects_primary_key_out_of_range() {
        let cols = vec![ColumnDef {
            name: "id".into(),
            ty: ColumnType::Integer,
            nullable: false,
        }];
        assert_eq!(
            Schema::new("t", cols, 5),
            Err(SchemaError::PrimaryKeyOutOfRange(5, 1))
        );
    }
}
