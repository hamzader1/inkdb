#[path = "common.rs"]
mod common;

use common::*;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use rusqlite::Connection;

fn read_all_texts(db: &mut Database<DiskVfs>, q: &str) -> Vec<String> {
    let mut statement = db.execute(q).expect("plan");
    let mut texts = Vec::new();
    for row in statement.rows() {
        let row = row.expect("row");
        for index in 0..row.len() {
            if let inkdb::record::Value::Text(text) = row.value(index).expect("value") {
                texts.push(text.into_owned());
            }
        }
    }
    texts
}

const PANICKING: [usize; 3] = [124, 125, 300];

#[test]
fn long_text_is_read_back_unchanged() {
    let path = db_path("longtextread");
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, body TEXT);")
        .expect("fixture ddl");
    for len in PANICKING {
        let value = "x".repeat(len);
        conn.execute("INSERT INTO t (body) VALUES (?1)", [&value])
            .expect("fixture row");
    }
    drop(conn);

    let mut db = open_engine(&path);
    let decoded = read_all_texts(&mut db, "select * from t");
    let expected: Vec<String> = PANICKING.iter().map(|len| "x".repeat(*len)).collect();
    assert_eq!(decoded, expected, "values must survive the read");
    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn long_text_written_by_ink_matches_the_reference() {
    let path = db_path("longtextwrite");
    let seed = Connection::open(&path).expect("seed");
    seed.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, body TEXT);")
        .expect("seed");
    drop(seed);

    let mut db = open_engine(&path);
    for len in PANICKING {
        let value = "y".repeat(len);
        run_ok(&mut db, &format!("insert into t (body) values ('{value}')"));
    }
    let written = read_all_texts(&mut db, "select * from t");
    let expected: Vec<String> = PANICKING.iter().map(|len| "y".repeat(*len)).collect();
    assert_eq!(written, expected, "what we wrote must be what we read");
    commit_and_close(db);

    let conn = Connection::open(&path).expect("reference");
    let mut statement = conn
        .prepare("SELECT body FROM t ORDER BY id")
        .expect("query");
    let from_reference: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("rows")
        .map(|row| row.expect("body"))
        .collect();
    assert_eq!(from_reference, expected, "the reference must read it too");
    drop(statement);
    drop(conn);
    cleanup(&path);
}

#[test]
fn a_long_ddl_is_readable_from_the_catalog() {
    let path = db_path("longtextcatalog");
    let seed = Connection::open(&path).expect("seed");
    seed.execute_batch("CREATE TABLE seed (x INTEGER);")
        .expect("seed");
    drop(seed);

    // A stored statement of 124 bytes has serial type 261, which is the value
    // that reads as an INT48 of six bytes. The name is padded so the statement
    // lands exactly on it.
    let name = format!("employees{}", "x".repeat(20));
    let ddl = format!(
        "CREATE TABLE {name} ( id INTEGER PRIMARY KEY, name TEXT UNIQUE, age INTEGER, salary REAL, city TEXT )"
    );
    assert_eq!(
        ddl.len(),
        124,
        "the padding must land on the reading that crashed"
    );

    let mut db = open_engine(&path);
    run_ok(&mut db, &ddl);

    let catalog = read_all_texts(&mut db, "select * from master");
    let stored = catalog
        .iter()
        .find(|text| text.starts_with("CREATE TABLE employees"))
        .expect("the stored DDL must survive the read");
    assert_eq!(
        stored.len(),
        ddl.len(),
        "the DDL came back short: {stored:?}"
    );
    assert_eq!(stored, &ddl);
    commit_and_close(db);
    cleanup(&path);
}
