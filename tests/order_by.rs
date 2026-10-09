#[path = "common.rs"]
mod common;

use common::*;
use inkdb::Master;
use inkdb::backend::analyzer::Analyze;
use inkdb::backend::planner::plan::Plan;
use inkdb::db::Database;
use inkdb::record::Value;
use inkdb::sql::lexer::Lexer;
use inkdb::sql::parser::Parser;
use inkdb::vfs::disk::DiskVfs;
use std::path::Path;
use std::rc::Rc;

fn sorted_column(db: &mut Database<DiskVfs>, q: &str, column: usize) -> Vec<Value<'static>> {
    let query: Rc<str> = Rc::from(q);
    let lexer = Lexer::tokenize(&query).expect("lex");
    let parsed = Parser::parse(Rc::clone(&query), lexer).expect("parse");
    let mut master = Master::new(db.pager()).expect("master");
    let resolved = Analyze::new(&master).analyze(parsed).expect("analyze");
    let mut plan = Plan::create_plan(resolved, db.pager(), &master).expect("plan");
    let mut out = Vec::new();
    loop {
        match plan.next(db.pager(), &mut master) {
            Ok(Some(row)) => out.push(row.value(column).expect("column").into_static()),
            Ok(None) => break,
            Err(e) => panic!("exec {q}: {e}"),
        }
    }
    out
}

fn reference_column(path: &Path, q: &str) -> Vec<Value<'static>> {
    let conn = rusqlite::Connection::open(path).expect("sqlite open");
    let mut stmt = conn.prepare(q).expect("prepare");
    let mut rows = stmt.query([]).expect("query");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("row") {
        let value = match row.get_ref(0).expect("value") {
            rusqlite::types::ValueRef::Integer(n) => Value::Integer(n),
            rusqlite::types::ValueRef::Real(f) => Value::Float(f),
            rusqlite::types::ValueRef::Text(t) => Value::Text(std::borrow::Cow::Owned(
                String::from_utf8_lossy(t).into_owned(),
            )),
            rusqlite::types::ValueRef::Null => Value::Null,
            other => panic!("unsupported reference value {other:?}"),
        };
        out.push(value);
    }
    out
}

fn check(
    db: &mut Database<DiskVfs>,
    path: &Path,
    engine_q: &str,
    reference_q: &str,
    column: usize,
) {
    let mine = sorted_column(db, engine_q, column);
    let theirs = reference_column(path, reference_q);
    assert_eq!(mine.len(), theirs.len(), "{engine_q}: row count");
    for (position, (got, want)) in mine.iter().zip(theirs.iter()).enumerate() {
        assert_eq!(got, want, "{engine_q}: first difference at row {position}");
    }
}

#[test]
fn order_by_matches_reference() {
    let path = db_path("sortscratch");
    build_users(&path, 4096, 4000);
    let mut db = open_engine(&path);

    check(
        &mut db,
        &path,
        "select * from users order by salary",
        "select salary from users order by salary",
        2,
    );
    check(
        &mut db,
        &path,
        "select * from users order by name",
        "select name from users order by name",
        0,
    );
    check(
        &mut db,
        &path,
        "select * from users order by age",
        "select age from users order by age",
        1,
    );
    check(
        &mut db,
        &path,
        "select * from users order by position",
        "select position from users order by position",
        3,
    );

    commit_and_close(db);
    cleanup(&path);

    let small = db_path("sortsmall");
    build_users(&small, 4096, 50);
    let mut db = open_engine(&small);
    check(
        &mut db,
        &small,
        "select * from users order by age",
        "select age from users order by age",
        1,
    );
    commit_and_close(db);
    cleanup(&small);
}

#[test]
fn order_by_keeps_the_rowid_of_an_integer_primary_key_table() {
    let path = db_path("orderrowid");
    let conn = rusqlite::Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);
         INSERT INTO t VALUES (11, 'c'), (22, 'a'), (33, 'b'), (40000000000, 'd');",
    )
    .expect("rows");
    drop(conn);

    let mut db = open_engine(&path);
    check(&mut db, &path, "select * from t", "select id from t", 0);
    check(
        &mut db,
        &path,
        "select * from t order by v",
        "select id from t order by v",
        0,
    );
    check(
        &mut db,
        &path,
        "select * from t order by v",
        "select v from t order by v",
        1,
    );
    // Sorting by the primary key column is sorting by the row id, which is not
    // stored in the row. The key of a row has to be read as if the column were
    // there, or the sort sees nothing to order by.
    check(
        &mut db,
        &path,
        "select * from t order by id",
        "select id from t order by id",
        0,
    );
    check(
        &mut db,
        &path,
        "select * from t order by id desc",
        "select id from t order by id desc",
        0,
    );
    commit_and_close(db);
    cleanup(&path);
}
