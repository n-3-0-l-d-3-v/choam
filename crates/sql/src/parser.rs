//! Recursive-descent statement parser with precedence climbing for
//! expressions (lowest to highest): OR, AND, NOT, comparison / IS NULL,
//! addition and subtraction, multiplication and division, unary minus,
//! primary. Keywords are case-insensitive and
//! reserved (they cannot be used as table or column names).

use row::ColumnType;

use crate::ast::*;
use crate::lexer::{lex, LexError, Token};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error(transparent)]
    Lex(#[from] LexError),
    #[error("expected {expected} at byte {pos}, found {found}")]
    Unexpected {
        found: String,
        expected: &'static str,
        pos: usize,
    },
    #[error("unexpected end of input, expected {expected}")]
    UnexpectedEnd { expected: &'static str },
    #[error("empty input")]
    Empty,
}

const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "PRIMARY", "KEY", "NOT", "NULL", "AND", "OR", "IS", "ORDER", "BY", "ASC", "DESC",
    "LIMIT", "BEGIN", "COMMIT", "ROLLBACK", "TRUE", "FALSE", "INTEGER", "TEXT", "BYTES", "BOOLEAN",
];

pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Parses exactly one statement, with an optional trailing `;`.
pub fn parse(sql: &str) -> Result<Statement, ParseError> {
    let mut stmts = parse_script(sql)?;
    match stmts.len() {
        0 => Err(ParseError::Empty),
        1 => Ok(stmts.remove(0)),
        _ => Err(ParseError::Unexpected {
            found: "another statement".into(),
            expected: "end of input",
            pos: 0,
        }),
    }
}

/// Parses zero or more `;`-separated statements.
pub fn parse_script(sql: &str) -> Result<Vec<Statement>, ParseError> {
    let tokens = lex(sql)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        end: sql.len(),
    };
    let mut out = Vec::new();
    loop {
        while p.eat(&Token::Semicolon) {}
        if p.peek().is_none() {
            break;
        }
        out.push(p.statement()?);
        match p.peek() {
            None | Some(Token::Semicolon) => {}
            Some(_) => return Err(p.unexpected("`;` or end of input")),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn unexpected(&self, expected: &'static str) -> ParseError {
        match self.tokens.get(self.pos) {
            Some((t, pos)) => ParseError::Unexpected {
                found: format!("{t:?}"),
                expected,
                pos: *pos,
            },
            None => ParseError::UnexpectedEnd { expected },
        }
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).map(|(t, _)| t.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Token) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: &Token, what: &'static str) -> Result<(), ParseError> {
        if self.eat(want) {
            Ok(())
        } else {
            Err(self.unexpected(what))
        }
    }

    fn at_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.eq_ignore_ascii_case(kw))
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self.at_keyword(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: &'static str) -> Result<(), ParseError> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            Err(self.unexpected(kw))
        }
    }

    fn ident(&mut self, what: &'static str) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::Word(w)) if !is_keyword(w) => {
                let w = w.clone();
                self.pos += 1;
                Ok(w)
            }
            _ => Err(self.unexpected(what)),
        }
    }

    fn ident_list(&mut self, what: &'static str) -> Result<Vec<String>, ParseError> {
        let mut v = vec![self.ident(what)?];
        while self.eat(&Token::Comma) {
            v.push(self.ident(what)?);
        }
        Ok(v)
    }

    fn statement(&mut self) -> Result<Statement, ParseError> {
        if self.eat_keyword("CREATE") {
            self.create_table()
        } else if self.eat_keyword("INSERT") {
            self.insert()
        } else if self.eat_keyword("SELECT") {
            self.select()
        } else if self.eat_keyword("UPDATE") {
            self.update()
        } else if self.eat_keyword("DELETE") {
            self.expect_keyword("FROM")?;
            let table = self.ident("table name")?;
            let filter = self.opt_where()?;
            Ok(Statement::Delete { table, filter })
        } else if self.eat_keyword("BEGIN") {
            Ok(Statement::Begin)
        } else if self.eat_keyword("COMMIT") {
            Ok(Statement::Commit)
        } else if self.eat_keyword("ROLLBACK") {
            Ok(Statement::Rollback)
        } else {
            Err(self.unexpected("a statement"))
        }
    }

    fn create_table(&mut self) -> Result<Statement, ParseError> {
        self.expect_keyword("TABLE")?;
        let name = self.ident("table name")?;
        self.expect(&Token::LParen, "`(`")?;
        let mut columns = Vec::new();
        loop {
            let cname = self.ident("column name")?;
            let ty = if self.eat_keyword("INTEGER") {
                ColumnType::Integer
            } else if self.eat_keyword("TEXT") {
                ColumnType::Text
            } else if self.eat_keyword("BYTES") {
                ColumnType::Bytes
            } else if self.eat_keyword("BOOLEAN") {
                ColumnType::Boolean
            } else {
                return Err(self.unexpected("a column type (INTEGER, TEXT, BYTES, BOOLEAN)"));
            };
            let (mut primary_key, mut not_null) = (false, false);
            loop {
                if self.eat_keyword("PRIMARY") {
                    self.expect_keyword("KEY")?;
                    primary_key = true;
                } else if self.eat_keyword("NOT") {
                    self.expect_keyword("NULL")?;
                    not_null = true;
                } else {
                    break;
                }
            }
            columns.push(ColumnSpec {
                name: cname,
                ty,
                primary_key,
                not_null,
            });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen, "`)` or `,`")?;
        Ok(Statement::CreateTable { name, columns })
    }

    fn insert(&mut self) -> Result<Statement, ParseError> {
        self.expect_keyword("INTO")?;
        let table = self.ident("table name")?;
        let columns = if self.eat(&Token::LParen) {
            let c = self.ident_list("column name")?;
            self.expect(&Token::RParen, "`)`")?;
            Some(c)
        } else {
            None
        };
        self.expect_keyword("VALUES")?;
        let mut rows = Vec::new();
        loop {
            self.expect(&Token::LParen, "`(`")?;
            let mut row = vec![self.expr()?];
            while self.eat(&Token::Comma) {
                row.push(self.expr()?);
            }
            self.expect(&Token::RParen, "`)` or `,`")?;
            rows.push(row);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(Statement::Insert {
            table,
            columns,
            rows,
        })
    }

    fn select(&mut self) -> Result<Statement, ParseError> {
        let projection = if self.eat(&Token::Star) {
            Projection::All
        } else {
            Projection::Columns(self.ident_list("column name or `*`")?)
        };
        self.expect_keyword("FROM")?;
        let table = self.ident("table name")?;
        let filter = self.opt_where()?;
        let order_by = if self.eat_keyword("ORDER") {
            self.expect_keyword("BY")?;
            let column = self.ident("column name")?;
            let descending = if self.eat_keyword("DESC") {
                true
            } else {
                self.eat_keyword("ASC");
                false
            };
            Some(OrderBy { column, descending })
        } else {
            None
        };
        let limit = if self.eat_keyword("LIMIT") {
            match self.advance() {
                Some(Token::Integer(n)) => Some(n as u64),
                _ => {
                    self.pos -= 1;
                    return Err(self.unexpected("an integer"));
                }
            }
        } else {
            None
        };
        Ok(Statement::Select {
            projection,
            table,
            filter,
            order_by,
            limit,
        })
    }

    fn update(&mut self) -> Result<Statement, ParseError> {
        let table = self.ident("table name")?;
        self.expect_keyword("SET")?;
        let mut assignments = Vec::new();
        loop {
            let col = self.ident("column name")?;
            self.expect(&Token::Eq, "`=`")?;
            assignments.push((col, self.expr()?));
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        let filter = self.opt_where()?;
        Ok(Statement::Update {
            table,
            assignments,
            filter,
        })
    }

    fn opt_where(&mut self) -> Result<Option<Expr>, ParseError> {
        if self.eat_keyword("WHERE") {
            Ok(Some(self.expr()?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn expr(&mut self) -> Result<Expr, ParseError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and_expr()?;
        while self.eat_keyword("OR") {
            let right = self.and_expr()?;
            left = Expr::Binary(Box::new(left), BinaryOp::Or, Box::new(right));
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.not_expr()?;
        while self.eat_keyword("AND") {
            let right = self.not_expr()?;
            left = Expr::Binary(Box::new(left), BinaryOp::And, Box::new(right));
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> Result<Expr, ParseError> {
        if self.eat_keyword("NOT") {
            Ok(Expr::Unary(UnaryOp::Not, Box::new(self.not_expr()?)))
        } else {
            self.cmp_expr()
        }
    }

    fn cmp_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.add_expr()?;
        loop {
            if self.eat_keyword("IS") {
                let negated = self.eat_keyword("NOT");
                self.expect_keyword("NULL")?;
                left = Expr::IsNull(Box::new(left), negated);
                continue;
            }
            let op = match self.peek() {
                Some(Token::Eq) => BinaryOp::Eq,
                Some(Token::Ne) => BinaryOp::Ne,
                Some(Token::Lt) => BinaryOp::Lt,
                Some(Token::Le) => BinaryOp::Le,
                Some(Token::Gt) => BinaryOp::Gt,
                Some(Token::Ge) => BinaryOp::Ge,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.add_expr()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
    }

    fn add_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinaryOp::Add,
                Some(Token::Minus) => BinaryOp::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.mul_expr()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
    }

    fn mul_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinaryOp::Mul,
                Some(Token::Slash) => BinaryOp::Div,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary_expr()?;
            left = Expr::Binary(Box::new(left), op, Box::new(right));
        }
    }

    fn unary_expr(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Token::Minus) {
            Ok(Expr::Unary(UnaryOp::Neg, Box::new(self.unary_expr()?)))
        } else {
            self.primary()
        }
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Some(Token::Integer(n)) => {
                let n = *n;
                self.pos += 1;
                Ok(Expr::Literal(Literal::Integer(n)))
            }
            Some(Token::Str(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(Expr::Literal(Literal::Text(s)))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(&Token::RParen, "`)`")?;
                Ok(e)
            }
            Some(Token::Word(w)) => {
                if w.eq_ignore_ascii_case("NULL") {
                    self.pos += 1;
                    Ok(Expr::Literal(Literal::Null))
                } else if w.eq_ignore_ascii_case("TRUE") {
                    self.pos += 1;
                    Ok(Expr::Literal(Literal::Boolean(true)))
                } else if w.eq_ignore_ascii_case("FALSE") {
                    self.pos += 1;
                    Ok(Expr::Literal(Literal::Boolean(false)))
                } else if is_keyword(w) {
                    Err(self.unexpected("an expression"))
                } else {
                    let w = w.clone();
                    self.pos += 1;
                    Ok(Expr::Column(w))
                }
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    #[allow(dead_code)]
    fn end_pos(&self) -> usize {
        self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(n: &str) -> Box<Expr> {
        Box::new(Expr::Column(n.into()))
    }
    fn int(n: i64) -> Box<Expr> {
        Box::new(Expr::Literal(Literal::Integer(n)))
    }

    #[test]
    fn create_table_with_modifiers_in_any_order() {
        let s = parse("create table Users (id integer not null primary key, name text)").unwrap();
        assert_eq!(
            s,
            Statement::CreateTable {
                name: "Users".into(),
                columns: vec![
                    ColumnSpec {
                        name: "id".into(),
                        ty: ColumnType::Integer,
                        primary_key: true,
                        not_null: true
                    },
                    ColumnSpec {
                        name: "name".into(),
                        ty: ColumnType::Text,
                        primary_key: false,
                        not_null: false
                    },
                ]
            }
        );
    }

    #[test]
    fn insert_multi_row_with_column_list() {
        let s = parse("INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL);").unwrap();
        let Statement::Insert {
            columns: Some(c),
            rows,
            ..
        } = s
        else {
            panic!()
        };
        assert_eq!(c, vec!["a", "b"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], Expr::Literal(Literal::Null));
    }

    #[test]
    fn select_with_every_clause() {
        let s = parse("SELECT a, b FROM t WHERE a > 1 ORDER BY b DESC LIMIT 5").unwrap();
        assert_eq!(
            s,
            Statement::Select {
                projection: Projection::Columns(vec!["a".into(), "b".into()]),
                table: "t".into(),
                filter: Some(Expr::Binary(col("a"), BinaryOp::Gt, int(1))),
                order_by: Some(OrderBy {
                    column: "b".into(),
                    descending: true
                }),
                limit: Some(5),
            }
        );
    }

    #[test]
    fn precedence_and_binds_tighter_than_or_and_mul_than_add() {
        let Statement::Delete {
            filter: Some(e), ..
        } = parse("DELETE FROM t WHERE a = 1 OR b = 2 AND c = 3 + 4 * 5").unwrap()
        else {
            panic!()
        };
        assert_eq!(
            e.to_string(),
            "((a = 1) OR ((b = 2) AND (c = (3 + (4 * 5)))))"
        );
    }

    #[test]
    fn subtraction_is_left_associative_and_neg_is_unary() {
        let Statement::Delete {
            filter: Some(e), ..
        } = parse("DELETE FROM t WHERE 10 - 3 - -2 = 9").unwrap()
        else {
            panic!()
        };
        assert_eq!(e.to_string(), "(((10 - 3) - (-2)) = 9)");
    }

    #[test]
    fn is_null_and_is_not_null_and_not() {
        let Statement::Delete {
            filter: Some(e), ..
        } = parse("DELETE FROM t WHERE NOT a IS NOT NULL").unwrap()
        else {
            panic!()
        };
        assert_eq!(e.to_string(), "(NOT (a IS NOT NULL))");
    }

    #[test]
    fn update_delete_and_transaction_control() {
        assert!(matches!(
            parse("UPDATE t SET a = a + 1, b = 'z' WHERE id = 3").unwrap(),
            Statement::Update { .. }
        ));
        assert_eq!(parse("begin").unwrap(), Statement::Begin);
        assert_eq!(parse("COMMIT;").unwrap(), Statement::Commit);
        assert_eq!(parse("Rollback").unwrap(), Statement::Rollback);
    }

    #[test]
    fn scripts_split_on_semicolons() {
        let v = parse_script("BEGIN; DELETE FROM t; ; COMMIT").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(parse_script("  ;; ").unwrap(), vec![]);
    }

    #[test]
    fn errors_are_typed_and_positioned() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert!(matches!(
            parse("SELECT FROM t"),
            Err(ParseError::Unexpected { pos: 7, .. })
        ));
        assert!(matches!(
            parse("SELECT * FROM"),
            Err(ParseError::UnexpectedEnd { .. })
        ));
        assert!(matches!(
            parse("SELECT * FROM t garbage"),
            Err(ParseError::Unexpected { .. })
        ));
        assert!(matches!(
            parse("SELECT * FROM t; SELECT * FROM u"),
            Err(ParseError::Unexpected { .. })
        ));
        assert!(matches!(parse("SELECT 'x"), Err(ParseError::Lex(_))));
    }

    #[test]
    fn keywords_cannot_be_identifiers() {
        assert!(parse("SELECT * FROM select").is_err());
        assert!(parse("CREATE TABLE t (key INTEGER)").is_err());
    }

    #[test]
    fn empty_lists_are_rejected() {
        assert!(parse("CREATE TABLE t ()").is_err());
        assert!(parse("INSERT INTO t VALUES ()").is_err());
        assert!(parse("INSERT INTO t () VALUES (1)").is_err());
    }

    #[test]
    fn limit_requires_an_integer() {
        assert!(parse("SELECT * FROM t LIMIT x").is_err());
        assert!(parse("SELECT * FROM t LIMIT -1").is_err());
    }
}
