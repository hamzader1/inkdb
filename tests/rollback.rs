#[path = "common.rs"]
mod common;

use common::*;
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use rusqlite::Connection;

fn run(db: &mut Database<DiskVfs>, q: &str) -> Result<usize, String> {
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
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);\n         INSERT INTO t VALUES (1, 'x'), (2, 'x');",
    )
    .expect("rows");
    drop(conn);
    path
}

#[test]
fn a_failed_statement_keeps_the_pages_of_committed_statements() {
    let path = fixture("rollbackgrow");
    let mut db = Database::new(&path).expect("open");

    run(&mut db, "create table t2 (id INTEGER PRIMARY KEY, a TEXT)").expect("create");
    run(&mut db, "insert into t2 values (1, 'a')").expect("insert");
    let before = std::fs::metadata(&path).expect("len").len();

    let err = run(&mut db, "create unique index t_v on t(v)")
        .expect_err("duplicate values must reject a unique index");
    assert!(err.to_ascii_lowercase().contains("unique"), "{err}");

    let after = std::fs::metadata(&path).expect("len").len();
    assert!(
        after >= before,
        "rollback truncated committed pages: {before} -> {after}"
    );

    assert_eq!(
        run(&mut db, "select * from t2").expect("t2 must survive the rollback"),
        1
    );

    drop(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn rows_written_by_committed_statements_survive_a_later_failure() {
    let path = fixture("rollbackrows");
    let mut db = Database::new(&path).expect("open");

    run(&mut db, "create table t3 (id INTEGER PRIMARY KEY, v TEXT)").expect("create");
    for i in 0..20 {
        run(&mut db, &format!("insert into t3 values ({i}, 'v{i}')")).expect("insert");
    }

    let err = run(&mut db, "create unique index t_v on t(v)")
        .expect_err("duplicate values must reject a unique index");
    assert!(err.to_ascii_lowercase().contains("unique"), "{err}");

    assert_eq!(
        run(&mut db, "select count(*) from t3").expect("t3 must survive"),
        1
    );
    assert_eq!(
        run(&mut db, "select * from t3").expect("t3 must survive"),
        20
    );

    drop(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn the_shell_path_keeps_the_session_after_an_error() {
    let path = fixture("rollbackshell");
    let mut db = Database::new(&path).expect("open");

    inkdb::shell::InkShell::test(&mut db, "create table s (id INTEGER PRIMARY KEY, v TEXT)")
        .expect("create");
    inkdb::shell::InkShell::test(&mut db, "insert into s values (1, 'a')").expect("insert");
    inkdb::shell::InkShell::test(&mut db, "insert into s values (2, 'b')").expect("insert");

    assert!(
        inkdb::shell::InkShell::test(&mut db, "create unique index t_v on t(v)").is_err(),
        "the failing statement must report an error"
    );

    inkdb::shell::InkShell::test(&mut db, "insert into s values (3, 'c')")
        .expect("insert after error");
    assert_eq!(
        run(&mut db, "select * from s").expect("s must survive the error"),
        3
    );

    drop(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}
