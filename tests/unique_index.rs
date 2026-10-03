#[path = "common.rs"]
mod common;

use common::*;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use rusqlite::Connection;

fn exec(db: &mut Database<DiskVfs>, q: &str) -> Result<usize, String> {
    let mut statement = db.execute(q).map_err(|e| e.to_string())?;
    let mut rows = 0;
    for row in statement.rows() {
        row.map_err(|e| e.to_string())?;
        rows += 1;
    }
    Ok(rows)
}

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
        message.to_lowercase().contains("unique") && message.contains("t1.name"),
        "expected a uniqueness error naming the column, got: {message}"
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

#[test]
fn a_created_table_gets_one_index_per_unique_column() {
    let path = db_path("uniauto");
    {
        let conn = Connection::open(&path).expect("seed");
        conn.execute_batch("CREATE TABLE seed (x INTEGER);")
            .expect("seed");
    }

    let mut db = Database::new(&path).expect("open");
    exec(
        &mut db,
        "create table t2 (id INTEGER PRIMARY KEY, name TEXT UNIQUE, email TEXT UNIQUE)",
    )
    .expect("create table");
    exec(&mut db, "insert into t2 (name, email) values ('a', 'a@x')").expect("insert");

    let duplicate_name = exec(&mut db, "insert into t2 (name, email) values ('a', 'b@x')")
        .expect_err("the name index must reject a duplicate");
    assert!(
        duplicate_name.to_ascii_lowercase().contains("unique"),
        "{duplicate_name}"
    );

    let duplicate_email = exec(&mut db, "insert into t2 (name, email) values ('b', 'a@x')")
        .expect_err("the email index must reject a duplicate");
    assert!(
        duplicate_email.to_ascii_lowercase().contains("unique"),
        "{duplicate_email}"
    );

    exec(&mut db, "insert into t2 (name, email) values ('b', 'b@x')")
        .expect("a distinct row must be accepted");
    drop(db);

    let conn = Connection::open(&path).expect("reference");
    let indexes: Vec<String> = conn
        .prepare(
            "select name from sqlite_master where type = 'index' \
             and name like 'ink_autoindex_t2_%' order by name",
        )
        .expect("query")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .map(|row| row.expect("name"))
        .collect();
    assert_eq!(
        indexes,
        vec![
            "ink_autoindex_t2_0".to_string(),
            "ink_autoindex_t2_1".to_string()
        ],
        "one index per unique column"
    );
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM t2", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 2, "only the two accepted rows");
    drop(conn);
    cleanup(&path);
}
