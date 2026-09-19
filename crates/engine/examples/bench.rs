//! Rough timings for the executor. Run with:
//!   cargo run --release -p engine --example bench
use std::time::Instant;

use engine::Engine;

fn time<T>(label: &str, n: usize, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let d = t.elapsed();
    println!(
        "{label:<44} {:>9.2?} total  {:>10.2?} per op",
        d,
        d / n as u32
    );
    r
}

fn main() {
    let dir = std::env::temp_dir().join(format!("choam-bench-{}", std::process::id()));
    let engine = Engine::open(&dir).unwrap();
    let mut s = engine.session();
    s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER, s TEXT)")
        .unwrap();

    time("autocommit INSERT (1 row = 1 fsync)", 200, || {
        for i in 0..200 {
            s.execute(&format!("INSERT INTO t VALUES ({i}, {i}, 'row{i}')"))
                .unwrap();
        }
    });
    let rows = 5_000;
    time("one transaction, INSERT 5000 rows", rows, || {
        s.execute("BEGIN").unwrap();
        for i in 200..200 + rows {
            s.execute(&format!("INSERT INTO t VALUES ({i}, {i}, 'row{i}')"))
                .unwrap();
        }
        s.execute("COMMIT").unwrap();
    });
    let total = 200 + rows;
    time("point lookup WHERE id = k (x2000)", 2000, || {
        for k in 0..2000 {
            s.execute(&format!("SELECT * FROM t WHERE id = {}", (k * 7) % total))
                .unwrap();
        }
    });
    time("full scan, filter on non-key (x20)", 20, || {
        for _ in 0..20 {
            let r = s.execute("SELECT id FROM t WHERE n > 4990").unwrap();
            assert!(!r.rows.is_empty());
        }
    });
    time("UPDATE every row (x1)", 1, || {
        s.execute("UPDATE t SET n = n + 1").unwrap();
    });
    time("full scan after 2 versions per row (x20)", 20, || {
        for _ in 0..20 {
            s.execute("SELECT id FROM t WHERE n > 4990").unwrap();
        }
    });
    drop(s);
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
