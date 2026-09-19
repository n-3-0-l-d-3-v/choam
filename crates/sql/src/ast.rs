//! The syntax tree for the supported SQL subset. `Display` renders a
//! statement back to SQL with every expression fully parenthesized, so
//! `parse(render(ast)) == ast` holds for any well-formed tree (checked by
//! a property test).

use std::fmt;

use row::ColumnType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Null,
    /// Always non-negative when produced by the parser; a negative number
    /// in source text is `Unary(Neg, Integer)`.
    Integer(i64),
    Text(String),
    Boolean(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Literal(Literal),
    Column(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(Box<Expr>, BinaryOp, Box<Expr>),
    /// `expr IS NULL` (`false`) or `expr IS NOT NULL` (`true` = negated).
    IsNull(Box<Expr>, bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: ColumnType,
    pub primary_key: bool,
    pub not_null: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Projection {
    All,
    Columns(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBy {
    pub column: String,
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    CreateTable {
        name: String,
        columns: Vec<ColumnSpec>,
    },
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        rows: Vec<Vec<Expr>>,
    },
    Select {
        projection: Projection,
        table: String,
        filter: Option<Expr>,
        order_by: Option<OrderBy>,
        limit: Option<u64>,
    },
    Update {
        table: String,
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
    },
    Delete {
        table: String,
        filter: Option<Expr>,
    },
    Begin,
    Commit,
    Rollback,
}

fn type_name(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
        ColumnType::Bytes => "BYTES",
        ColumnType::Boolean => "BOOLEAN",
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Null => write!(f, "NULL"),
            Literal::Integer(n) => write!(f, "{n}"),
            Literal::Text(s) => write!(f, "'{}'", s.replace('\'', "''")),
            Literal::Boolean(true) => write!(f, "TRUE"),
            Literal::Boolean(false) => write!(f, "FALSE"),
        }
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        })
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Literal(l) => write!(f, "{l}"),
            Expr::Column(c) => write!(f, "{c}"),
            Expr::Unary(UnaryOp::Neg, e) => write!(f, "(-{e})"),
            Expr::Unary(UnaryOp::Not, e) => write!(f, "(NOT {e})"),
            Expr::Binary(l, op, r) => write!(f, "({l} {op} {r})"),
            Expr::IsNull(e, false) => write!(f, "({e} IS NULL)"),
            Expr::IsNull(e, true) => write!(f, "({e} IS NOT NULL)"),
        }
    }
}

fn join<T: fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::CreateTable { name, columns } => {
                let cols: Vec<String> = columns
                    .iter()
                    .map(|c| {
                        let mut s = format!("{} {}", c.name, type_name(c.ty));
                        if c.primary_key {
                            s.push_str(" PRIMARY KEY");
                        }
                        if c.not_null {
                            s.push_str(" NOT NULL");
                        }
                        s
                    })
                    .collect();
                write!(f, "CREATE TABLE {name} ({})", cols.join(", "))
            }
            Statement::Insert {
                table,
                columns,
                rows,
            } => {
                write!(f, "INSERT INTO {table}")?;
                if let Some(cols) = columns {
                    write!(f, " ({})", cols.join(", "))?;
                }
                let rows: Vec<String> = rows.iter().map(|r| format!("({})", join(r))).collect();
                write!(f, " VALUES {}", rows.join(", "))
            }
            Statement::Select {
                projection,
                table,
                filter,
                order_by,
                limit,
            } => {
                match projection {
                    Projection::All => write!(f, "SELECT *")?,
                    Projection::Columns(c) => write!(f, "SELECT {}", c.join(", "))?,
                }
                write!(f, " FROM {table}")?;
                if let Some(w) = filter {
                    write!(f, " WHERE {w}")?;
                }
                if let Some(o) = order_by {
                    write!(
                        f,
                        " ORDER BY {} {}",
                        o.column,
                        if o.descending { "DESC" } else { "ASC" }
                    )?;
                }
                if let Some(n) = limit {
                    write!(f, " LIMIT {n}")?;
                }
                Ok(())
            }
            Statement::Update {
                table,
                assignments,
                filter,
            } => {
                let sets: Vec<String> = assignments
                    .iter()
                    .map(|(c, e)| format!("{c} = {e}"))
                    .collect();
                write!(f, "UPDATE {table} SET {}", sets.join(", "))?;
                if let Some(w) = filter {
                    write!(f, " WHERE {w}")?;
                }
                Ok(())
            }
            Statement::Delete { table, filter } => {
                write!(f, "DELETE FROM {table}")?;
                if let Some(w) = filter {
                    write!(f, " WHERE {w}")?;
                }
                Ok(())
            }
            Statement::Begin => write!(f, "BEGIN"),
            Statement::Commit => write!(f, "COMMIT"),
            Statement::Rollback => write!(f, "ROLLBACK"),
        }
    }
}
