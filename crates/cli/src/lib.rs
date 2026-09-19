//! The `choamc` SQL shell: reads statements (terminated by `;`) and dot
//! commands from any reader, runs them on an `engine::Session`, and
//! prints results. Works the same for a terminal, a pipe or a file.
//!
//! Dot commands: `.tables`, `.schema [table]`, `.help`, `.quit`.

use std::io::{self, BufRead, Write};

use engine::{Engine, QueryResult, Session};
use row::{ColumnType, Value};
use sql::Statement;

fn cell(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Integer(n) => n.to_string(),
        Value::Text(s) => s.clone(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Bytes(b) => format!(
            "x'{}'",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
    }
}

/// Renders a SELECT result as an aligned text table with a row count.
pub fn format_table(r: &QueryResult) -> String {
    let body: Vec<Vec<String>> = r
        .rows
        .iter()
        .map(|row| row.iter().map(cell).collect())
        .collect();
    let widths: Vec<usize> = r
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            body.iter()
                .map(|row| row[i].chars().count())
                .chain([c.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join(" | ")
            .trim_end()
            .to_string()
    };
    let mut out = String::new();
    out += &line(&r.columns);
    out.push('\n');
    out += &widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join("-+-");
    out.push('\n');
    for row in &body {
        out += &line(row);
        out.push('\n');
    }
    out += &format!(
        "({} row{})\n",
        body.len(),
        if body.len() == 1 { "" } else { "s" }
    );
    out
}

fn type_name(t: ColumnType) -> &'static str {
    match t {
        ColumnType::Integer => "INTEGER",
        ColumnType::Text => "TEXT",
        ColumnType::Bytes => "BYTES",
        ColumnType::Boolean => "BOOLEAN",
    }
}

fn describe(t: &catalog::TableHandle) -> String {
    let cols: Vec<String> = t
        .schema
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut s = format!("{} {}", c.name, type_name(c.ty));
            if i == t.schema.primary_key {
                s += " PRIMARY KEY";
            } else if !c.nullable {
                s += " NOT NULL";
            }
            s
        })
        .collect();
    format!("{} ({})", t.schema.table_name, cols.join(", "))
}

/// Splits the first complete `;`-terminated statement off `buf`, ignoring
/// semicolons inside string literals. Returns `(statement, rest)`.
fn split_statement(buf: &str) -> Option<(&str, &str)> {
    let mut in_str = false;
    for (i, ch) in buf.char_indices() {
        match ch {
            '\'' => in_str = !in_str,
            ';' if !in_str => return Some((&buf[..i], &buf[i + 1..])),
            _ => {}
        }
    }
    None
}

struct Shell<'a, 'e, W: Write, E: Write> {
    engine: &'e Engine,
    session: &'a mut Session<'e>,
    out: &'a mut W,
    err: &'a mut E,
    had_error: bool,
}

impl<W: Write, E: Write> Shell<'_, '_, W, E> {
    fn statement(&mut self, text: &str) -> io::Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let stmt = match sql::parse(text) {
            Ok(s) => s,
            Err(e) => return self.fail(e),
        };
        match self.session.execute_statement(&stmt) {
            Err(e) => self.fail(e),
            Ok(r) => match stmt {
                Statement::Select { .. } => write!(self.out, "{}", format_table(&r)),
                Statement::Insert { .. } | Statement::Update { .. } | Statement::Delete { .. } => {
                    writeln!(
                        self.out,
                        "{} row{} affected",
                        r.rows_affected,
                        if r.rows_affected == 1 { "" } else { "s" }
                    )
                }
                _ => writeln!(self.out, "OK"),
            },
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) -> io::Result<()> {
        self.had_error = true;
        writeln!(self.err, "error: {e}")
    }

    /// Returns false when the shell should exit.
    fn dot(&mut self, line: &str) -> io::Result<bool> {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some(".quit") | Some(".exit") => return Ok(false),
            Some(".help") => writeln!(
                self.out,
                "SQL statements end with `;`. Dot commands: .tables  .schema [table]  .help  .quit"
            )?,
            Some(".tables") => match self.engine.database().list_tables() {
                Ok(ts) => {
                    for t in ts {
                        writeln!(self.out, "{}", t.schema.table_name)?;
                    }
                }
                Err(e) => self.fail(e)?,
            },
            Some(".schema") => match self.engine.database().list_tables() {
                Ok(ts) => {
                    let want = parts.next();
                    let mut found = false;
                    for t in ts
                        .iter()
                        .filter(|t| want.is_none_or(|w| t.schema.table_name == w))
                    {
                        found = true;
                        writeln!(self.out, "{}", describe(t))?;
                    }
                    if !found {
                        if let Some(w) = want {
                            self.fail(format!("no such table {w:?}"))?;
                        }
                    }
                }
                Err(e) => self.fail(e)?,
            },
            _ => self.fail(format!("unknown command {line:?} (try .help)"))?,
        }
        Ok(true)
    }
}

/// Runs everything readable from `input`. Returns true if any statement or
/// command failed (the shell keeps going after errors). `prompt` prints
/// `choam> ` before each line read, for interactive terminals.
pub fn run<'e, R: BufRead, W: Write, E: Write>(
    engine: &'e Engine,
    session: &mut Session<'e>,
    mut input: R,
    out: &mut W,
    err: &mut E,
    prompt: bool,
) -> io::Result<bool> {
    let mut shell = Shell {
        engine,
        session,
        out,
        err,
        had_error: false,
    };
    let mut buf = String::new();
    let mut line = String::new();
    loop {
        if prompt {
            write!(
                shell.out,
                "{}",
                if buf.trim().is_empty() {
                    "choam> "
                } else {
                    "   ...> "
                }
            )?;
            shell.out.flush()?;
        }
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        if buf.trim().is_empty() && line.trim_start().starts_with('.') {
            buf.clear();
            if !shell.dot(line.trim())? {
                return Ok(shell.had_error);
            }
            continue;
        }
        buf.push_str(&line);
        while let Some((stmt, rest)) = split_statement(&buf) {
            let (stmt, rest) = (stmt.to_string(), rest.to_string());
            shell.statement(&stmt)?;
            buf = rest;
        }
    }
    if !buf.trim().is_empty() {
        shell.statement(&buf)?;
    }
    Ok(shell.had_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_formatting_aligns_columns_and_counts_rows() {
        let r = QueryResult {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Value::Integer(1), Value::Text("alice".into())],
                vec![Value::Integer(20), Value::Null],
            ],
            rows_affected: 0,
        };
        assert_eq!(
            format_table(&r),
            "id | name\n---+------\n1  | alice\n20 | NULL\n(2 rows)\n"
        );
    }

    #[test]
    fn one_row_is_singular() {
        let r = QueryResult {
            columns: vec!["a".into()],
            rows: vec![vec![Value::Boolean(true)]],
            rows_affected: 0,
        };
        assert!(format_table(&r).ends_with("(1 row)\n"));
    }

    #[test]
    fn statements_split_on_semicolons_outside_strings() {
        assert_eq!(split_statement("a; b"), Some(("a", " b")));
        assert_eq!(split_statement("x = 'a;b'; c"), Some(("x = 'a;b'", " c")));
        assert_eq!(split_statement("no terminator"), None);
        assert_eq!(split_statement("'it''s;'; z"), Some(("'it''s;'", " z")));
    }
}
