//! Differential test (ticket 004): arbitrary sequences of SQL statements
//! (including BEGIN/COMMIT/ROLLBACK and deliberately failing statements)
//! run through `engine::Session` and through an independent reference
//! model written directly against a `BTreeMap`, with its own three-valued
//! predicate evaluation that shares no code with the engine. Every
//! statement must agree on success versus failure, on rows returned, and
//! on rows affected; the final table contents must agree too.
//!
//! Table under test: t(id INTEGER PRIMARY KEY, n INTEGER, s TEXT, b BOOLEAN).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use engine::Engine;
use proptest::prelude::*;
use row::Value;

#[derive(Debug, Clone, Copy)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn sql(self) -> &'static str {
        match self {
            Cmp::Eq => "=",
            Cmp::Ne => "<>",
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        }
    }
    fn holds(self, o: Ordering) -> bool {
        match self {
            Cmp::Eq => o == Ordering::Equal,
            Cmp::Ne => o != Ordering::Equal,
            Cmp::Lt => o == Ordering::Less,
            Cmp::Le => o != Ordering::Greater,
            Cmp::Gt => o == Ordering::Greater,
            Cmp::Ge => o != Ordering::Less,
        }
    }
}

#[derive(Debug, Clone)]
enum P {
    N(Cmp, i64),
    Id(Cmp, i64),
    S(String),
    B(bool),
    NNull(bool),
    And(Box<P>, Box<P>),
    Or(Box<P>, Box<P>),
    Not(Box<P>),
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl P {
    fn sql(&self) -> String {
        match self {
            P::N(c, k) => format!("(n {} {k})", c.sql()),
            P::Id(c, k) => format!("(id {} {k})", c.sql()),
            P::S(t) => format!("(s = {})", quote(t)),
            P::B(v) => format!("(b = {})", if *v { "TRUE" } else { "FALSE" }),
            P::NNull(neg) => format!("(n IS {}NULL)", if *neg { "NOT " } else { "" }),
            P::And(a, b) => format!("({} AND {})", a.sql(), b.sql()),
            P::Or(a, b) => format!("({} OR {})", a.sql(), b.sql()),
            P::Not(a) => format!("(NOT {})", a.sql()),
        }
    }

    fn eval(&self, id: i64, r: &R) -> Option<bool> {
        match self {
            P::N(c, k) => r.n.map(|n| c.holds(n.cmp(k))),
            P::Id(c, k) => Some(c.holds(id.cmp(k))),
            P::S(t) => r.s.as_ref().map(|s| s == t),
            P::B(v) => r.b.map(|b| b == *v),
            P::NNull(neg) => Some(r.n.is_none() != *neg),
            P::And(a, b) => match (a.eval(id, r), b.eval(id, r)) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            },
            P::Or(a, b) => match (a.eval(id, r), b.eval(id, r)) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
            P::Not(a) => a.eval(id, r).map(|v| !v),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct R {
    n: Option<i64>,
    s: Option<String>,
    b: Option<bool>,
}

type Model = BTreeMap<i64, R>;

fn values(id: i64, r: &R) -> Vec<Value> {
    vec![
        Value::Integer(id),
        r.n.map_or(Value::Null, Value::Integer),
        r.s.clone().map_or(Value::Null, Value::Text),
        r.b.map_or(Value::Null, Value::Boolean),
    ]
}

type InsRow = (i64, Option<i64>, Option<String>, Option<bool>);

#[derive(Debug, Clone)]
enum Op {
    Insert(Vec<InsRow>),
    UpdateN(i64, Option<P>),
    UpdateS(String, Option<P>),
    Delete(Option<P>),
    Select(Vec<usize>, Option<P>, Option<(usize, bool)>, Option<u64>),
    BadSelect,
    BadUpdate,
    Begin,
    Commit,
    Rollback,
}

const COLS: [&str; 4] = ["id", "n", "s", "b"];

fn opt_lit<T>(v: &Option<T>, f: impl Fn(&T) -> String) -> String {
    v.as_ref().map_or("NULL".to_string(), f)
}

fn where_sql(p: &Option<P>) -> String {
    p.as_ref()
        .map_or(String::new(), |p| format!(" WHERE {}", p.sql()))
}

impl Op {
    fn sql(&self) -> String {
        match self {
            Op::Insert(rows) => {
                let rs: Vec<String> = rows
                    .iter()
                    .map(|(id, n, s, b)| {
                        format!(
                            "({id}, {}, {}, {})",
                            opt_lit(n, |v| v.to_string()),
                            opt_lit(s, |v| quote(v)),
                            opt_lit(b, |v| if *v { "TRUE".into() } else { "FALSE".into() })
                        )
                    })
                    .collect();
                format!("INSERT INTO t VALUES {}", rs.join(", "))
            }
            Op::UpdateN(k, p) => format!("UPDATE t SET n = n + {k}{}", where_sql(p)),
            Op::UpdateS(s, p) => format!("UPDATE t SET s = {}{}", quote(s), where_sql(p)),
            Op::Delete(p) => format!("DELETE FROM t{}", where_sql(p)),
            Op::Select(cols, p, order, limit) => {
                let cs: Vec<&str> = cols.iter().map(|&i| COLS[i]).collect();
                let mut q = format!("SELECT {} FROM t{}", cs.join(", "), where_sql(p));
                if let Some((c, desc)) = order {
                    q += &format!(
                        " ORDER BY {} {}",
                        COLS[*c],
                        if *desc { "DESC" } else { "ASC" }
                    );
                }
                if let Some(l) = limit {
                    q += &format!(" LIMIT {l}");
                }
                q
            }
            Op::BadSelect => "SELECT * FROM t WHERE n".into(),
            Op::BadUpdate => "UPDATE t SET id = 1".into(),
            Op::Begin => "BEGIN".into(),
            Op::Commit => "COMMIT".into(),
            Op::Rollback => "ROLLBACK".into(),
        }
    }
}

#[derive(Debug, PartialEq)]
struct Outcome {
    rows: Vec<Vec<Value>>,
    affected: usize,
}

#[derive(Default)]
struct Reference {
    data: Model,
    backup: Option<Model>,
    failed: bool,
}

fn apply(data: &mut Model, op: &Op) -> Result<Outcome, ()> {
    let keep =
        |p: &Option<P>, id: i64, r: &R| p.as_ref().is_none_or(|p| p.eval(id, r) == Some(true));
    match op {
        Op::Insert(rows) => {
            for (id, n, s, b) in rows {
                if data.contains_key(id) {
                    return Err(());
                }
                data.insert(
                    *id,
                    R {
                        n: *n,
                        s: s.clone(),
                        b: *b,
                    },
                );
            }
            Ok(Outcome {
                rows: vec![],
                affected: rows.len(),
            })
        }
        Op::UpdateN(k, p) => {
            let mut c = 0;
            for (id, r) in data.iter_mut() {
                if keep(p, *id, r) {
                    r.n = r.n.map(|v| v + k);
                    c += 1;
                }
            }
            Ok(Outcome {
                rows: vec![],
                affected: c,
            })
        }
        Op::UpdateS(s, p) => {
            let mut c = 0;
            for (id, r) in data.iter_mut() {
                if keep(p, *id, r) {
                    r.s = Some(s.clone());
                    c += 1;
                }
            }
            Ok(Outcome {
                rows: vec![],
                affected: c,
            })
        }
        Op::Delete(p) => {
            let doomed: Vec<i64> = data
                .iter()
                .filter(|(id, r)| keep(p, **id, r))
                .map(|(id, _)| *id)
                .collect();
            for id in &doomed {
                data.remove(id);
            }
            Ok(Outcome {
                rows: vec![],
                affected: doomed.len(),
            })
        }
        Op::Select(cols, p, order, limit) => {
            let mut rows: Vec<Vec<Value>> = data
                .iter()
                .filter(|(id, r)| keep(p, **id, r))
                .map(|(id, r)| values(*id, r))
                .collect();
            if let Some((c, desc)) = order {
                rows.sort_by(|a, b| {
                    let o = match (&a[*c], &b[*c]) {
                        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
                        (Value::Text(x), Value::Text(y)) => x.cmp(y),
                        (Value::Boolean(x), Value::Boolean(y)) => x.cmp(y),
                        (Value::Null, Value::Null) => Ordering::Equal,
                        (Value::Null, _) => Ordering::Less,
                        (_, Value::Null) => Ordering::Greater,
                        _ => unreachable!(),
                    };
                    if *desc {
                        o.reverse()
                    } else {
                        o
                    }
                });
            }
            if let Some(l) = limit {
                rows.truncate(*l as usize);
            }
            let rows = rows
                .into_iter()
                .map(|r| cols.iter().map(|&i| r[i].clone()).collect())
                .collect();
            Ok(Outcome { rows, affected: 0 })
        }
        Op::BadSelect | Op::BadUpdate => Err(()),
        Op::Begin | Op::Commit | Op::Rollback => unreachable!(),
    }
}

impl Reference {
    fn step(&mut self, op: &Op) -> Result<Outcome, ()> {
        let none = || Outcome {
            rows: vec![],
            affected: 0,
        };
        match op {
            Op::Begin => {
                if self.backup.is_some() {
                    return Err(());
                }
                self.backup = Some(self.data.clone());
                Ok(none())
            }
            Op::Commit => {
                let bk = self.backup.take().ok_or(())?;
                if std::mem::take(&mut self.failed) {
                    self.data = bk;
                    return Err(());
                }
                Ok(none())
            }
            Op::Rollback => {
                self.data = self.backup.take().ok_or(())?;
                self.failed = false;
                Ok(none())
            }
            _ => {
                if self.failed {
                    return Err(());
                }
                let mut work = self.data.clone();
                match apply(&mut work, op) {
                    Ok(o) => {
                        self.data = work;
                        Ok(o)
                    }
                    Err(()) => {
                        self.failed = self.backup.is_some();
                        Err(())
                    }
                }
            }
        }
    }
}

fn arb_cmp() -> impl Strategy<Value = Cmp> {
    prop_oneof![
        Just(Cmp::Eq),
        Just(Cmp::Ne),
        Just(Cmp::Lt),
        Just(Cmp::Le),
        Just(Cmp::Gt),
        Just(Cmp::Ge)
    ]
}

fn arb_text() -> impl Strategy<Value = String> {
    "[a-c']{0,3}"
}

fn arb_p() -> impl Strategy<Value = P> {
    let leaf = prop_oneof![
        (arb_cmp(), -6i64..6).prop_map(|(c, k)| P::N(c, k)),
        (arb_cmp(), -1i64..8).prop_map(|(c, k)| P::Id(c, k)),
        // Bias toward exact-id equality so the point-lookup path runs often.
        (0i64..8).prop_map(|k| P::Id(Cmp::Eq, k)),
        arb_text().prop_map(P::S),
        any::<bool>().prop_map(P::B),
        any::<bool>().prop_map(P::NNull),
    ];
    leaf.prop_recursive(3, 12, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| P::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| P::Or(Box::new(a), Box::new(b))),
            inner.prop_map(|a| P::Not(Box::new(a))),
        ]
    })
}

fn arb_op() -> impl Strategy<Value = Op> {
    let ins_row = (
        0i64..8,
        prop::option::of(-5i64..5),
        prop::option::of(arb_text()),
        prop::option::of(any::<bool>()),
    );
    prop_oneof![
        4 => prop::collection::vec(ins_row, 1..4).prop_map(Op::Insert),
        2 => (-5i64..5, prop::option::of(arb_p())).prop_map(|(k, p)| Op::UpdateN(k, p)),
        1 => (arb_text(), prop::option::of(arb_p())).prop_map(|(s, p)| Op::UpdateS(s, p)),
        2 => prop::option::of(arb_p()).prop_map(Op::Delete),
        5 => (
            prop::collection::vec(0usize..4, 1..4),
            prop::option::of(arb_p()),
            prop::option::of((0usize..4, any::<bool>())),
            prop::option::of(0u64..5),
        ).prop_map(|(c, p, o, l)| Op::Select(c, p, o, l)),
        1 => Just(Op::BadSelect),
        1 => Just(Op::BadUpdate),
        2 => Just(Op::Begin),
        2 => Just(Op::Commit),
        1 => Just(Op::Rollback),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn engine_matches_the_in_memory_reference_model(ops in prop::collection::vec(arb_op(), 0..40)) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        let mut s = engine.session();
        s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT, b BOOLEAN)").unwrap();
        let mut reference = Reference::default();

        for op in &ops {
            let sql = op.sql();
            let real = s.execute(&sql);
            let model = reference.step(op);
            match (real, model) {
                (Ok(r), Ok(m)) => {
                    prop_assert_eq!(r.rows, m.rows, "rows differ for: {}", sql);
                    prop_assert_eq!(r.rows_affected, m.affected, "rows_affected differs for: {}", sql);
                }
                (Err(_), Err(())) => {}
                (real, model) => prop_assert!(false, "outcome differs for `{}`: engine {:?}, model {:?}", sql, real.map(|_| "ok"), model.map(|_| "ok")),
            }
        }

        if reference.backup.is_some() {
            s.execute("ROLLBACK").unwrap();
            reference.step(&Op::Rollback).unwrap();
        }
        let all = s.execute("SELECT * FROM t").unwrap();
        let expected: Vec<Vec<Value>> = reference.data.iter().map(|(id, r)| values(*id, r)).collect();
        prop_assert_eq!(all.rows, expected);
    }
}
