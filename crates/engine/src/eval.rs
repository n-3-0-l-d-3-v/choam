//! Expression evaluation with SQL's three-valued logic: any operation on
//! NULL yields NULL, except IS [NOT] NULL, AND and OR (where
//! `FALSE AND NULL` is FALSE and `TRUE OR NULL` is TRUE). Both operands of
//! AND/OR are always evaluated (no short-circuit), so an error in either
//! side is reported deterministically.

use std::cmp::Ordering;

use row::{ColumnType, Schema, Value};
use sql::{BinaryOp, Expr, Literal, UnaryOp};

use crate::ExecError;

pub type RowCtx<'a> = Option<(&'a Schema, &'a [Value])>;

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NULL",
        Value::Integer(_) => "INTEGER",
        Value::Text(_) => "TEXT",
        Value::Bytes(_) => "BYTES",
        Value::Boolean(_) => "BOOLEAN",
    }
}

fn mismatch(op: &str, a: &Value, b: Option<&Value>) -> ExecError {
    ExecError::TypeMismatch(match b {
        Some(b) => format!("{op} cannot combine {} and {}", type_name(a), type_name(b)),
        None => format!("{op} cannot be applied to {}", type_name(a)),
    })
}

pub fn eval(expr: &Expr, ctx: RowCtx<'_>) -> Result<Value, ExecError> {
    match expr {
        Expr::Literal(l) => Ok(match l {
            Literal::Null => Value::Null,
            Literal::Integer(n) => Value::Integer(*n),
            Literal::Text(s) => Value::Text(s.clone()),
            Literal::Boolean(b) => Value::Boolean(*b),
        }),
        Expr::Column(name) => {
            let (schema, row) = ctx.ok_or_else(|| ExecError::UnknownColumn(name.clone()))?;
            let i = schema
                .columns
                .iter()
                .position(|c| &c.name == name)
                .ok_or_else(|| ExecError::UnknownColumn(name.clone()))?;
            Ok(row[i].clone())
        }
        Expr::IsNull(e, negated) => {
            let v = eval(e, ctx)?;
            Ok(Value::Boolean(matches!(v, Value::Null) != *negated))
        }
        Expr::Unary(op, e) => {
            let v = eval(e, ctx)?;
            match (op, v) {
                (_, Value::Null) => Ok(Value::Null),
                (UnaryOp::Neg, Value::Integer(n)) => n
                    .checked_neg()
                    .map(Value::Integer)
                    .ok_or(ExecError::IntegerOverflow),
                (UnaryOp::Not, Value::Boolean(b)) => Ok(Value::Boolean(!b)),
                (UnaryOp::Neg, other) => Err(mismatch("unary -", &other, None)),
                (UnaryOp::Not, other) => Err(mismatch("NOT", &other, None)),
            }
        }
        Expr::Binary(l, op, r) => {
            let a = eval(l, ctx)?;
            let b = eval(r, ctx)?;
            binary(*op, a, b)
        }
    }
}

fn binary(op: BinaryOp, a: Value, b: Value) -> Result<Value, ExecError> {
    use BinaryOp::*;
    match op {
        And | Or => {
            let as_bool = |v: &Value| match v {
                Value::Boolean(x) => Ok(Some(*x)),
                Value::Null => Ok(None),
                other => Err(mismatch(if op == And { "AND" } else { "OR" }, other, None)),
            };
            let (x, y) = (as_bool(&a)?, as_bool(&b)?);
            let out = if op == And {
                match (x, y) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }
            } else {
                match (x, y) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }
            };
            Ok(out.map_or(Value::Null, Value::Boolean))
        }
        Add | Sub | Mul | Div => {
            if matches!(a, Value::Null) || matches!(b, Value::Null) {
                return Ok(Value::Null);
            }
            let (Value::Integer(x), Value::Integer(y)) = (&a, &b) else {
                return Err(mismatch("arithmetic", &a, Some(&b)));
            };
            let r = match op {
                Add => x.checked_add(*y),
                Sub => x.checked_sub(*y),
                Mul => x.checked_mul(*y),
                _ => {
                    if *y == 0 {
                        return Err(ExecError::DivisionByZero);
                    }
                    x.checked_div(*y)
                }
            };
            r.map(Value::Integer).ok_or(ExecError::IntegerOverflow)
        }
        Eq | Ne | Lt | Le | Gt | Ge => {
            if matches!(a, Value::Null) || matches!(b, Value::Null) {
                return Ok(Value::Null);
            }
            let ord = compare(&a, &b).ok_or_else(|| mismatch("comparison", &a, Some(&b)))?;
            Ok(Value::Boolean(match op {
                Eq => ord == Ordering::Equal,
                Ne => ord != Ordering::Equal,
                Lt => ord == Ordering::Less,
                Le => ord != Ordering::Greater,
                Gt => ord == Ordering::Greater,
                _ => ord != Ordering::Less,
            }))
        }
    }
}

/// Orders two non-null values of the same type; `None` if the types differ.
pub fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Some(x.cmp(y)),
        (Value::Text(x), Value::Text(y)) => Some(x.cmp(y)),
        (Value::Bytes(x), Value::Bytes(y)) => Some(x.cmp(y)),
        (Value::Boolean(x), Value::Boolean(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// The static type of an expression; `Null` is the type of a bare NULL and
/// is compatible with everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Null,
    Integer,
    Text,
    Bytes,
    Boolean,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Null => "NULL",
            Ty::Integer => "INTEGER",
            Ty::Text => "TEXT",
            Ty::Bytes => "BYTES",
            Ty::Boolean => "BOOLEAN",
        }
    }
}

impl From<ColumnType> for Ty {
    fn from(t: ColumnType) -> Ty {
        match t {
            ColumnType::Integer => Ty::Integer,
            ColumnType::Text => Ty::Text,
            ColumnType::Bytes => Ty::Bytes,
            ColumnType::Boolean => Ty::Boolean,
        }
    }
}

fn is(t: Ty, want: Ty) -> bool {
    t == want || t == Ty::Null
}

/// Type-checks `expr` against `schema` without touching any row, so a
/// statement's type errors do not depend on which rows happen to exist
/// (`WHERE n` must fail on an empty table just as on a full one). Data-
/// dependent errors (overflow, division by zero) remain runtime errors.
pub fn check_type(expr: &Expr, schema: Option<&Schema>) -> Result<Ty, ExecError> {
    let bad = |what: String| Err(ExecError::TypeMismatch(what));
    match expr {
        Expr::Literal(l) => Ok(match l {
            Literal::Null => Ty::Null,
            Literal::Integer(_) => Ty::Integer,
            Literal::Text(_) => Ty::Text,
            Literal::Boolean(_) => Ty::Boolean,
        }),
        Expr::Column(name) => {
            let schema = schema.ok_or_else(|| ExecError::UnknownColumn(name.clone()))?;
            schema
                .columns
                .iter()
                .find(|c| &c.name == name)
                .map(|c| Ty::from(c.ty))
                .ok_or_else(|| ExecError::UnknownColumn(name.clone()))
        }
        Expr::IsNull(e, _) => {
            check_type(e, schema)?;
            Ok(Ty::Boolean)
        }
        Expr::Unary(op, e) => {
            let t = check_type(e, schema)?;
            let want = if *op == UnaryOp::Neg {
                Ty::Integer
            } else {
                Ty::Boolean
            };
            if is(t, want) {
                Ok(want)
            } else {
                bad(format!("unary operator cannot be applied to {}", t.name()))
            }
        }
        Expr::Binary(l, op, r) => {
            let (a, b) = (check_type(l, schema)?, check_type(r, schema)?);
            use BinaryOp::*;
            match op {
                And | Or => {
                    if is(a, Ty::Boolean) && is(b, Ty::Boolean) {
                        Ok(Ty::Boolean)
                    } else {
                        bad(format!(
                            "AND/OR cannot combine {} and {}",
                            a.name(),
                            b.name()
                        ))
                    }
                }
                Add | Sub | Mul | Div => {
                    if is(a, Ty::Integer) && is(b, Ty::Integer) {
                        Ok(Ty::Integer)
                    } else {
                        bad(format!(
                            "arithmetic cannot combine {} and {}",
                            a.name(),
                            b.name()
                        ))
                    }
                }
                _ => {
                    if a == b || a == Ty::Null || b == Ty::Null {
                        Ok(Ty::Boolean)
                    } else {
                        bad(format!(
                            "comparison cannot combine {} and {}",
                            a.name(),
                            b.name()
                        ))
                    }
                }
            }
        }
    }
}

/// A WHERE clause must be BOOLEAN (or a bare NULL).
pub fn check_filter(filter: Option<&Expr>, schema: &Schema) -> Result<(), ExecError> {
    if let Some(f) = filter {
        let t = check_type(f, Some(schema))?;
        if !is(t, Ty::Boolean) {
            return Err(ExecError::TypeMismatch(format!(
                "WHERE must be boolean, got {}",
                t.name()
            )));
        }
    }
    Ok(())
}

/// A WHERE clause keeps a row only if it evaluates to exactly TRUE (FALSE
/// and NULL both drop it); any other type is an error.
pub fn keeps_row(filter: Option<&Expr>, schema: &Schema, row: &[Value]) -> Result<bool, ExecError> {
    let Some(f) = filter else { return Ok(true) };
    match eval(f, Some((schema, row)))? {
        Value::Boolean(b) => Ok(b),
        Value::Null => Ok(false),
        other => Err(ExecError::TypeMismatch(format!(
            "WHERE must be boolean, got {}",
            type_name(&other)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sql::{parse, Statement};

    fn e(text: &str) -> Result<Value, ExecError> {
        let Statement::Delete {
            filter: Some(f), ..
        } = parse(&format!("DELETE FROM t WHERE {text}")).unwrap()
        else {
            panic!()
        };
        eval(&f, None)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(e("1 + 2 * 3"), Ok(Value::Integer(7)));
        assert_eq!(e("7 / 2"), Ok(Value::Integer(3)));
        assert_eq!(e("-7 / 2"), Ok(Value::Integer(-3)));
    }

    #[test]
    fn overflow_and_division_by_zero_are_errors() {
        assert_eq!(e("1 / 0"), Err(ExecError::DivisionByZero));
        assert_eq!(
            e("9223372036854775807 + 1"),
            Err(ExecError::IntegerOverflow)
        );
        assert_eq!(
            e("(-9223372036854775807 - 1) / -1"),
            Err(ExecError::IntegerOverflow)
        );
        assert_eq!(
            e("-(-9223372036854775807 - 1)"),
            Err(ExecError::IntegerOverflow)
        );
    }

    #[test]
    fn null_propagates_through_comparison_and_arithmetic() {
        assert_eq!(e("NULL = NULL"), Ok(Value::Null));
        assert_eq!(e("1 + NULL"), Ok(Value::Null));
        assert_eq!(e("NULL IS NULL"), Ok(Value::Boolean(true)));
        assert_eq!(e("1 IS NOT NULL"), Ok(Value::Boolean(true)));
    }

    #[test]
    fn three_valued_and_or() {
        assert_eq!(e("FALSE AND NULL"), Ok(Value::Boolean(false)));
        assert_eq!(e("TRUE AND NULL"), Ok(Value::Null));
        assert_eq!(e("TRUE OR NULL"), Ok(Value::Boolean(true)));
        assert_eq!(e("FALSE OR NULL"), Ok(Value::Null));
        assert_eq!(e("NOT NULL"), Ok(Value::Null));
    }

    #[test]
    fn type_mismatches_are_errors_not_coercions() {
        assert!(matches!(e("1 = 'a'"), Err(ExecError::TypeMismatch(_))));
        assert!(matches!(e("'a' + 1"), Err(ExecError::TypeMismatch(_))));
        assert!(matches!(e("NOT 1"), Err(ExecError::TypeMismatch(_))));
        assert!(matches!(e("1 AND TRUE"), Err(ExecError::TypeMismatch(_))));
    }

    #[test]
    fn text_and_boolean_ordering() {
        assert_eq!(e("'a' < 'b'"), Ok(Value::Boolean(true)));
        assert_eq!(e("FALSE < TRUE"), Ok(Value::Boolean(true)));
        assert_eq!(e("'b' <= 'a'"), Ok(Value::Boolean(false)));
    }

    #[test]
    fn column_without_a_row_is_unknown() {
        assert_eq!(e("x = 1"), Err(ExecError::UnknownColumn("x".into())));
    }
}
