//! Closing workload (ticket 005): concurrent bank transfers through SQL.
//! Each transfer is a read-modify-write inside BEGIN..COMMIT: read both
//! balances, write both back. Threads contend on a handful of accounts and
//! retry on `Conflict`. A lost update (two transfers reading the same
//! balance and both overwriting it) would break the conservation
//! invariant; first-committer-wins must prevent it.

use std::sync::atomic::{AtomicUsize, Ordering};

use engine::{Engine, ExecError, Session};
use row::Value;
use txn::DbError;

const ACCOUNTS: i64 = 4;
const INITIAL: i64 = 1_000;

fn balance(s: &mut Session<'_>, id: i64) -> i64 {
    let r = s
        .execute(&format!("SELECT balance FROM accounts WHERE id = {id}"))
        .unwrap();
    match r.rows[0][0] {
        Value::Integer(n) => n,
        ref other => panic!("{other:?}"),
    }
}

fn transfer(s: &mut Session<'_>, from: i64, to: i64, amount: i64) -> Result<(), ExecError> {
    s.execute("BEGIN")?;
    let (a, b) = (balance(s, from), balance(s, to));
    s.execute(&format!(
        "UPDATE accounts SET balance = {} WHERE id = {from}",
        a - amount
    ))?;
    s.execute(&format!(
        "UPDATE accounts SET balance = {} WHERE id = {to}",
        b + amount
    ))?;
    s.execute("COMMIT").map(|_| ())
}

#[test]
fn concurrent_transfers_conserve_money_and_never_lose_an_update() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    {
        let mut s = engine.session();
        s.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)")
            .unwrap();
        for id in 0..ACCOUNTS {
            s.execute(&format!("INSERT INTO accounts VALUES ({id}, {INITIAL})"))
                .unwrap();
        }
    }

    let threads = 6;
    let per_thread = 25;
    let conflicts = AtomicUsize::new(0);
    // Net change per account contributed by committed transfers, per thread.
    let deltas: Vec<Vec<i64>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let (engine, conflicts) = (&engine, &conflicts);
                scope.spawn(move || {
                    let mut s = engine.session();
                    let mut delta = vec![0i64; ACCOUNTS as usize];
                    for j in 0..per_thread {
                        let from = (t + j) % ACCOUNTS as usize;
                        let to = (from + 1 + j % 3) % ACCOUNTS as usize;
                        let amount = 1 + (j % 5) as i64;
                        loop {
                            match transfer(&mut s, from as i64, to as i64, amount) {
                                Ok(()) => {
                                    delta[from] -= amount;
                                    delta[to] += amount;
                                    break;
                                }
                                Err(ExecError::Db(DbError::Conflict { .. })) => {
                                    conflicts.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(e) => panic!("unexpected: {e}"),
                            }
                        }
                    }
                    delta
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut s = engine.session();
    let mut total = 0;
    for id in 0..ACCOUNTS {
        let expected = INITIAL + deltas.iter().map(|d| d[id as usize]).sum::<i64>();
        let actual = balance(&mut s, id);
        assert_eq!(
            actual, expected,
            "account {id}: a committed transfer was lost or duplicated"
        );
        total += actual;
    }
    assert_eq!(total, ACCOUNTS * INITIAL, "money must be conserved");
    eprintln!("conflicts retried: {}", conflicts.load(Ordering::Relaxed));
}
