//! End-to-end tests of the real `choamc` binary over a real directory.

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn choamc(dir: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_choamc"));
    cmd.arg("--dir").arg(dir).args(args);
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

#[test]
fn a_session_over_stdin_prints_tables_and_counts() {
    let d = tempfile::tempdir().unwrap();
    let o = choamc(
        d.path(),
        &[],
        Some(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);\n\
             INSERT INTO users VALUES (1, 'ada', 36), (2, 'grace', NULL);\n\
             SELECT * FROM users ORDER BY id;\n\
             UPDATE users SET age = 85 WHERE id = 2;\n\
             SELECT name, age FROM users WHERE age > 40;\n",
        ),
    );
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(
        text(&o.stdout),
        "OK\n2 rows affected\n\
         id | name  | age\n---+-------+----\n1  | ada   | 36\n2  | grace | NULL\n(2 rows)\n\
         1 row affected\n\
         name  | age\n------+----\ngrace | 85\n(1 row)\n"
    );
}

#[test]
fn data_persists_across_separate_processes() {
    let d = tempfile::tempdir().unwrap();
    assert!(choamc(
        d.path(),
        &[
            "-c",
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (7, 'kept')"
        ],
        None
    )
    .status
    .success());
    let o = choamc(d.path(), &["-c", "SELECT v FROM t WHERE id = 7"], None);
    assert!(text(&o.stdout).contains("kept"));
}

#[test]
fn errors_go_to_stderr_exit_1_and_the_shell_keeps_going() {
    let d = tempfile::tempdir().unwrap();
    let o = choamc(
        d.path(),
        &[],
        Some("SELEC nonsense;\nCREATE TABLE t (id INTEGER PRIMARY KEY);\nINSERT INTO t VALUES (1);\nINSERT INTO t VALUES (1);\nSELECT * FROM t;\n"),
    );
    assert_eq!(o.status.code(), Some(1));
    let (out, err) = (text(&o.stdout), text(&o.stderr));
    assert!(
        err.contains("error:") && err.contains("duplicate primary key"),
        "{err}"
    );
    assert!(out.contains("(1 row)"), "later statements still ran: {out}");
}

#[test]
fn transactions_and_dot_commands() {
    let d = tempfile::tempdir().unwrap();
    let o = choamc(
        d.path(),
        &[],
        Some("CREATE TABLE a (id INTEGER PRIMARY KEY, x TEXT NOT NULL);\n.tables\n.schema a\nBEGIN;\nINSERT INTO a VALUES (1, 'x');\nROLLBACK;\nSELECT * FROM a;\n.quit\nSELECT 1;\n"),
    );
    let out = text(&o.stdout);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        out.contains("a (id INTEGER PRIMARY KEY, x TEXT NOT NULL)"),
        "{out}"
    );
    assert!(
        out.contains("(0 rows)"),
        "rollback discarded the insert: {out}"
    );
}

#[test]
fn semicolons_inside_strings_do_not_split_statements() {
    let d = tempfile::tempdir().unwrap();
    let o = choamc(d.path(), &["-c", "CREATE TABLE t (id INTEGER PRIMARY KEY, s TEXT); INSERT INTO t VALUES (1, 'a;b'); SELECT s FROM t"], None);
    assert!(text(&o.stdout).contains("a;b"));
}

#[test]
fn bad_arguments_exit_2() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(choamc(d.path(), &["--nope"], None).status.code(), Some(2));
}
