#[path = "common.rs"]
mod common;

use common::*;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn fixture(tag: &str, sql: &str) -> PathBuf {
    let path = db_path(tag);
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(sql).expect("fixture sql");
    drop(conn);
    path
}

fn catalog(path: &Path, where_clause: &str) -> i64 {
    let conn = Connection::open(path).expect("reference");
    conn.query_row(
        &format!("SELECT COUNT(*) FROM sqlite_master WHERE {where_clause}"),
        [],
        |r| r.get(0),
    )
    .expect("catalog query")
}

fn page_stats(path: &Path) -> (i64, i64) {
    let conn = Connection::open(path).expect("reference");
    let pages: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .expect("page_count");
    let free: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .expect("freelist_count");
    (pages, free)
}

#[test]
fn dropping_a_table_removes_its_catalog_rows_and_frees_its_pages() {
    let path = fixture(
        "droptable",
        "CREATE TABLE t (a TEXT, b INT);
         INSERT INTO t VALUES ('x', 1), ('y', 2);
         CREATE INDEX t_a ON t(a);",
    );
    assert_eq!(catalog(&path, "name IN ('t', 't_a')"), 2, "fixture shape");

    let mut db = open_engine(&path);
    run_ok(&mut db, "drop table t");
    commit_and_close(db);

    assert_eq!(catalog(&path, "name IN ('t', 't_a')"), 0, "catalog rows");
    let (pages, free) = page_stats(&path);
    assert_eq!(pages, 3, "page count is unchanged");
    assert_eq!(free, 2, "the table page and the index page are free");
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn dropping_a_table_leaves_other_tables_readable() {
    let path = fixture(
        "dropneighbour",
        "CREATE TABLE keep (a TEXT);
         INSERT INTO keep VALUES ('one'), ('two'), ('three');
         CREATE TABLE gone (a TEXT);
         INSERT INTO gone VALUES ('x');",
    );

    let mut db = open_engine(&path);
    run_ok(&mut db, "drop table gone");
    assert_eq!(run_count(&mut db, "select * from keep"), 3, "kept rows");
    commit_and_close(db);

    assert_eq!(catalog(&path, "name = 'gone'"), 0, "dropped");
    assert_eq!(catalog(&path, "name = 'keep'"), 1, "not dropped");
    assert_integrity_ok(&path);

    let conn = Connection::open(&path).expect("reference");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM keep", [], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 3, "the reference sees the kept rows");
    drop(conn);
    cleanup(&path);
}

#[test]
fn dropping_an_index_removes_only_that_index() {
    let path = fixture(
        "dropindex",
        "CREATE TABLE t (a TEXT, b INT);
         INSERT INTO t VALUES ('x', 1), ('y', 2), ('z', 3);
         CREATE INDEX first ON t(a);
         CREATE INDEX second ON t(b);",
    );
    assert_eq!(catalog(&path, "type = 'index'"), 2, "fixture shape");

    let mut db = open_engine(&path);
    run_ok(&mut db, "drop index first");
    assert_eq!(run_count(&mut db, "select * from t"), 3, "rows untouched");
    commit_and_close(db);

    assert_eq!(catalog(&path, "name = 'first'"), 0, "dropped");
    assert_eq!(catalog(&path, "name = 'second'"), 1, "not dropped");
    assert_eq!(catalog(&path, "name = 't'"), 1, "the table stays");
    let (pages, free) = page_stats(&path);
    assert_eq!(pages, 4, "master, table, and the two index roots");
    assert_eq!(free, 1, "one index tree came back to the freelist");
    assert_integrity_ok(&path);

    let conn = Connection::open(&path).expect("reference");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .expect("count");
    assert_eq!(rows, 3, "the reference sees every row");
    let via_index: i64 = conn
        .query_row("SELECT COUNT(*) FROM t WHERE b >= 2", [], |r| r.get(0))
        .expect("count");
    assert_eq!(via_index, 2, "the surviving index is usable");
    drop(conn);
    cleanup(&path);
}

#[test]
fn dropping_a_missing_table_or_index_is_an_error() {
    let path = fixture("dropmissing", "CREATE TABLE t (a TEXT);");
    let mut db = open_engine(&path);

    let table = run_err(&mut db, "drop table nope");
    assert!(table.contains("does not exist"), "{table}");
    let index = run_err(&mut db, "drop index nope");
    assert!(index.contains("no such index"), "{index}");

    commit_and_close(db);
    assert_eq!(catalog(&path, "name = 't'"), 1, "nothing was dropped");
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn drop_needs_a_target() {
    let path = fixture("dropjunk", "CREATE TABLE t (a TEXT);");
    let mut db = open_engine(&path);

    let bare = run_err(&mut db, "drop");
    assert!(bare.contains("Expected TABLE or INDEX"), "{bare}");
    let other = run_err(&mut db, "drop view v");
    assert!(other.contains("Expected TABLE or INDEX"), "{other}");

    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn a_dropped_table_can_be_created_and_used_again() {
    let path = fixture(
        "dropreuse",
        "CREATE TABLE t (a TEXT, b INT);
         INSERT INTO t VALUES ('x', 1);",
    );

    let mut db = open_engine(&path);
    run_ok(&mut db, "drop table t");
    run_ok(&mut db, "create table t (a TEXT, b INT)");
    run_ok(&mut db, "insert into t values ('fresh', 9)");
    let rows = run_count(&mut db, "select * from t");
    assert_eq!(rows, 1, "the recreated table holds only the new row");
    commit_and_close(db);

    assert_eq!(catalog(&path, "name = 't'"), 1, "one table named t");
    assert_integrity_ok(&path);

    let conn = Connection::open(&path).expect("reference");
    let (a, b): (String, i64) = conn
        .query_row("SELECT a, b FROM t", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("row");
    assert_eq!(
        (a.as_str(), b),
        ("fresh", 9),
        "the reference sees the new row"
    );
    drop(conn);
    cleanup(&path);
}

#[test]
fn dropping_a_table_created_by_ink_takes_its_automatic_indexes_with_it() {
    let path = fixture("dropauto", "CREATE TABLE seed (x INTEGER);");

    let mut db = open_engine(&path);
    run_ok(
        &mut db,
        "create table t (id integer primary key, name text unique)",
    );
    commit_and_close(db);
    assert_eq!(
        catalog(&path, "tbl_name = 't'"),
        2,
        "the table and its index"
    );

    let mut db = open_engine(&path);
    run_ok(&mut db, "drop table t");
    commit_and_close(db);

    assert_eq!(catalog(&path, "name = 't'"), 0, "the table is gone");
    assert_eq!(catalog(&path, "tbl_name = 't'"), 0, "no row still names it");
    assert_eq!(
        catalog(&path, "type = 'index'"),
        0,
        "the index went with it"
    );
    assert_integrity_ok(&path);
    cleanup(&path);
}
