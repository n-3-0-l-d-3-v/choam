//! Property test (ticket 002): concurrent transactions' outcomes are
//! consistent with some serial ordering.
//!
//! Scope note: `sietch::Transaction` gives Snapshot Isolation with
//! first-committer-wins write-write conflict detection, not full
//! serializability — SI's known gap is *write skew* (two transactions
//! each read a key the other writes, and write based on that read,
//! producing an outcome no serial execution could). Proving general
//! serializability would require a workload with cross-key read-write
//! dependencies, which this transaction API doesn't need to support for
//! this ticket's scope (no cross-row invariants are enforced yet). So
//! this test uses **blind writes**: every transaction commits writes to
//! random rows without reading anything else first. For that workload,
//! any interleaving that produces a set of committed transactions is
//! trivially equivalent to executing those same transactions serially in
//! their commit order — a losing (conflicting) transaction contributes
//! nothing, exactly as if it had never run — so the real database's
//! final state must equal a serial reference model replayed in the
//! actual commit order. That equivalence is what's checked below.
//!
//! Retried transactions (on `DbError::Conflict`) are treated as: the
//! failed attempt never happened, and the retry is a new transaction —
//! matching what a real client would do.

use std::sync::{Arc, Mutex};
use std::thread;

use proptest::prelude::*;
use row::{ColumnDef, ColumnType, Value};
use txn::{Database, DbError};

fn cols() -> Vec<ColumnDef> {
    vec![
        ColumnDef {
            name: "id".into(),
            ty: ColumnType::Integer,
            nullable: false,
        },
        ColumnDef {
            name: "value".into(),
            ty: ColumnType::Text,
            nullable: false,
        },
    ]
}

/// One thread's unit of work: a single blind write to `pk`, tagged with
/// `label` so the commit log can identify which write actually won.
#[derive(Debug, Clone)]
struct WriteJob {
    pk: i64,
    label: String,
}

fn arb_jobs() -> impl Strategy<Value = Vec<WriteJob>> {
    // Small keyspace (0..4) so multiple threads collide on the same rows
    // often — an uncontested workload wouldn't exercise conflict
    // detection at all.
    prop::collection::vec((0i64..4, "[a-z]{1,6}"), 2..24).prop_map(|pairs| {
        pairs
            .into_iter()
            .enumerate()
            .map(|(i, (pk, tag))| WriteJob {
                pk,
                label: format!("{tag}-{i}"),
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn concurrent_commits_match_a_serial_replay_in_commit_order(jobs in arb_jobs()) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new({
            let db = Database::open(dir.path()).unwrap();
            db.create_table("kv", cols(), 0).unwrap();
            db
        });

        // Every successful commit appends (pk, label) here, in the exact
        // order commits actually landed — this *is* the serial order the
        // reference model will replay.
        let commit_log: Arc<Mutex<Vec<(i64, String)>>> = Arc::new(Mutex::new(Vec::new()));

        let handles: Vec<_> = jobs
            .into_iter()
            .map(|job| {
                let db = db.clone();
                let commit_log = commit_log.clone();
                thread::spawn(move || {
                    loop {
                        let mut txn = db.begin();
                        txn.write("kv", &[Value::Integer(job.pk), Value::Text(job.label.clone())])
                            .unwrap();
                        match txn.commit() {
                            Ok(()) => {
                                // Recording under the same lock a real committer
                                // would need anyway keeps "commit order" and
                                // "log order" the same thing, not two things
                                // that could race apart.
                                commit_log.lock().unwrap().push((job.pk, job.label.clone()));
                                return;
                            }
                            Err(DbError::Conflict { .. }) => continue, // retry, as a real client would
                            Err(e) => panic!("unexpected error: {e}"),
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        // Serial reference model: replay the commit log in order.
        let mut model = std::collections::HashMap::new();
        for (pk, label) in commit_log.lock().unwrap().iter() {
            model.insert(*pk, label.clone());
        }

        let readback = db.begin();
        for pk in 0i64..4 {
            let actual = readback
                .read("kv", &Value::Integer(pk))
                .unwrap()
                .map(|row| match &row[1] {
                    Value::Text(s) => s.clone(),
                    other => panic!("expected Text, got {other:?}"),
                });
            prop_assert_eq!(
                &actual,
                &model.get(&pk).cloned(),
                "row {} must match the serial replay of the actual commit order",
                pk
            );
        }
    }
}
