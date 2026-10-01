//! UNIQUE index: duplicate NULLs are allowed (SQLite treats NULLs as
//! distinct), real duplicates are rejected, and the index is usable afterwards.

#[path = "common.rs"]
mod common;

use common::*;
use rusqlite::Connection;

fn fixture(tag: &str) -> std::path::PathBuf {
    let path = db_path(tag);
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE t1 (name TEXT, age INT);\n         INSERT INTO t1 VALUES ('alpha', 1), (NULL, 2), (NULL, 3), ('beta', 4);",
    )
    .expect("rows");
    drop(conn);
    path
}

#[test]
fn unique_index_allows_duplicate_nulls() {
    let path = fixture("uniqnulls");
    let mut db = open_engine(&path);

    run_ok(&mut db, "create unique index alpha on t1(name)");

    commit_and_close(db);
    let conn = Connection::open(&path).expect("reference");
    let found: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'alpha'",
            [],
            |row| row.get(0),
        )
        .expect("index present");
    assert_eq!(found, 1, "the unique index should exist in sqlite_master");
    drop(conn);

    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn unique_index_rejects_a_real_duplicate() {
    let path = db_path("uniqdup");
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE t1 (name TEXT, age INT);
         INSERT INTO t1 VALUES ('alpha', 1), (NULL, 2), ('alpha', 3);",
    )
    .expect("rows");
    drop(conn);
    let mut db = open_engine(&path);

    let message = run_err(&mut db, "create unique index alpha on t1(name)");
    assert!(
        message.contains("unique"),
        "expected a uniqueness error, got: {message}"
    );

    let conn = Connection::open(&path).expect("reference");
    let built: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'alpha'",
            [],
            |row| row.get(0),
        )
        .expect("index lookup");
    assert_eq!(built, 0, "a rejected index must not exist");
    drop(conn);

    cleanup(&path);
}

#[test]
fn unique_index_build_is_usable_afterwards() {
    let path = db_path("uniquse");
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE t1 (name TEXT, age INT);\n         INSERT INTO t1 VALUES ('alpha', 1), (NULL, 2), ('beta', 4), ('gamma', 5);",
    )
    .expect("rows");
    drop(conn);

    let mut db = open_engine(&path);
    run_ok(&mut db, "create unique index alpha on t1(name)");
    assert_eq!(
        run_count(&mut db, "select * from t1 where name = 'beta'"),
        1
    );
    assert_eq!(run_count(&mut db, "select * from t1"), 4);
    commit_and_close(db);

    let conn = Connection::open(&path).expect("reference");
    let found: i64 = conn
        .query_row("SELECT COUNT(*) FROM t1 WHERE name = 'beta'", [], |row| {
            row.get(0)
        })
        .expect("count");
    assert_eq!(
        found, 1,
        "the reference engine must find the row through the index we built"
    );
    drop(conn);
    assert_integrity_ok(&path);
    cleanup(&path);
}
