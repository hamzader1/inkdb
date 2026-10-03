//! INSERT semantics against the reference engine: column lists (including
//! reordered and partial), NOT NULL, arity, and unknown names.

#[path = "common.rs"]
mod common;

use common::*;
use rusqlite::Connection;

fn fixture(tag: &str) -> std::path::PathBuf {
    let path = db_path(tag);
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch("CREATE TABLE t1 (a INT NOT NULL, b TEXT, c INT);")
        .expect("table");
    drop(conn);
    path
}

#[test]
fn column_lists_resolve_to_the_right_columns() {
    let path = fixture("insertcols");
    let mut db = open_engine(&path);

    run_ok(&mut db, "insert into t1 (a, b, c) values (1, 'x', 2)");
    run_ok(&mut db, "insert into t1 (c, a, b) values (9, 3, 'y')");
    run_ok(&mut db, "insert into t1 (a, b, c) values (5, null, null)");
    run_ok(&mut db, "insert into t1 values (4, 'w', 7)");
    commit_and_close(db);

    let conn = Connection::open(&path).expect("reference");
    let mut statement = conn
        .prepare("SELECT a, b, c FROM t1 ORDER BY a")
        .expect("prepare");
    let engine: Vec<(i64, Option<String>, Option<i64>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(
        engine,
        vec![
            (1, Some("x".into()), Some(2)),
            (3, Some("y".into()), Some(9)),
            (4, Some("w".into()), Some(7)),
            (5, None, None),
        ]
    );
    drop(statement);
    drop(conn);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn arity_and_unknown_columns_are_rejected() {
    let path = fixture("insertbad");
    let mut db = open_engine(&path);

    let message = run_err(&mut db, "insert into t1 (a, b, c) values (5)");
    assert_eq!(message, "1 values for 3 columns", "SQLite's wording");

    let message = run_err(&mut db, "insert into t1 values (1, 'x', 2, 9)");
    assert_eq!(message, "4 values for 3 columns");

    let message = run_err(&mut db, "insert into t1 (a, zz) values (1, 2)");
    assert_eq!(message, "table t1 has no column named zz");

    commit_and_close(db);
    let conn = Connection::open(&path).expect("reference");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM t1", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 0, "nothing may be inserted by a rejected statement");
    drop(conn);
    cleanup(&path);
}

#[test]
fn not_null_and_type_mismatch_are_rejected() {
    let path = fixture("insertnull");
    let mut db = open_engine(&path);

    let message = run_err(&mut db, "insert into t1 (b) values ('z')");
    assert_eq!(message, "NOT NULL constraint failed: t1.a");

    let message = run_err(&mut db, "insert into t1 (a, b, c) values (null, 'q', 1)");
    assert_eq!(message, "NOT NULL constraint failed: t1.a");

    let message = run_err(&mut db, "insert into t1 (a, b) values (1, 2)");
    assert!(
        message.contains("Type mismatch on column 'b'"),
        "unexpected message: {message}"
    );

    commit_and_close(db);
    cleanup(&path);
}
