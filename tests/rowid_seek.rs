//! An INTEGER PRIMARY KEY is the table b-tree key, so a comparison on it is a
//! seek or a range walk on the table itself, not a scan. Results must match
//! the reference.

#[path = "common.rs"]
mod common;

use common::*;
use inkdb::Master;
use inkdb::backend::analyzer::Analyze;
use inkdb::backend::planner::plan::{Plan, PlanTree};
use inkdb::db::Database;
use inkdb::vfs::disk::DiskVfs;
use rusqlite::Connection;
use std::rc::Rc;

fn plan_text(db: &mut Database<DiskVfs>, q: &str) -> String {
    let query: Rc<str> = Rc::from(q);
    let lexer = inkdb::sql::lexer::Lexer::tokenize(&query).expect("lex");
    let parsed = inkdb::sql::parser::Parser::parse(Rc::clone(&query), lexer).expect("parse");
    let master = Master::new(&mut db.pager).expect("master");
    let resolved = Analyze::new(&master).analyze(parsed).expect("analyze");
    let prepared = Plan::create_plan(resolved, &mut db.pager, &master).expect("plan");
    let table = prepared.table_name().and_then(|name| master.table(name));
    format!(
        "{}",
        PlanTree::new(&prepared.parent, &prepared.arena, table)
    )
}

fn fixture(tag: &str) -> std::path::PathBuf {
    let path = db_path(tag);
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE k (id INTEGER PRIMARY KEY, v TEXT);\n         INSERT INTO k VALUES (1, 'a'), (2, 'b'), (500, 'c'), (70000, 'd');",
    )
    .expect("rows");
    drop(conn);
    path
}

#[test]
fn an_integer_primary_key_is_a_tree_seek() {
    let path = fixture("rowidseek");
    let mut db = open_engine(&path);

    let text = plan_text(&mut db, "select * from k where id = 2");
    assert!(
        text.contains("RowRangeScan"),
        "expected a rowid seek: {text}"
    );
    assert!(
        !text.contains("TableScan"),
        "a rowid seek must not scan: {text}"
    );

    assert!(plan_text(&mut db, "select count(*) from k where id = 500").contains("RowRangeScan"));
    assert!(
        plan_text(&mut db, "select * from k where id > 2").contains("RowRangeScan"),
        "a comparison on the key column maps onto a rowid range"
    );
    assert!(
        plan_text(&mut db, "select * from k where id >= 2 and id < 20").contains("RowRangeScan"),
        "a bounded interval maps onto one rowid range"
    );
    assert!(
        !plan_text(&mut db, "select * from k where v = 'b'").contains("RowRangeScan"),
        "a non-key column must still scan"
    );

    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn rowid_seek_returns_the_same_rows_as_the_reference() {
    let path = fixture("rowidrows");
    let mut db = open_engine(&path);

    for (id, expected) in [
        (1, 1),
        (2, 1),
        (500, 1),
        (70000, 1),
        (0, 0),
        (3, 0),
        (99999, 0),
    ] {
        let q = format!("select * from k where id = {id}");
        assert_eq!(run_count(&mut db, &q), expected, "{q}");
        let conn = Connection::open(&path).expect("reference");
        let reference: i64 = conn
            .query_row(&q.replace("select *", "SELECT COUNT(*)"), [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(reference, expected as i64, "reference disagrees on {q}");
        drop(conn);
    }

    assert_eq!(
        run_count(&mut db, "select count(*) from k where id = 70000"),
        1
    );

    commit_and_close(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn rowid_ranges_return_the_same_rows_as_the_reference() {
    let path = fixture("rowidrange");
    let mut db = open_engine(&path);

    let queries = [
        "select * from k where id > 2",
        "select * from k where id >= 2",
        "select * from k where id < 500",
        "select * from k where id <= 500",
        "select * from k where id > 1 and id < 70000",
        "select * from k where id >= 2 and id <= 500",
        "select * from k where id > 2 and id > 400",
        "select * from k where id > 1 and id < 70000 and id > 400",
        "select * from k where id < 2",
        "select * from k where id > 70000",
        "select * from k where id > 2 and id < 500 and v = 'c'",
    ];

    for q in queries {
        let ink = run_count(&mut db, q);
        let conn = Connection::open(&path).expect("reference");
        let reference: i64 = conn
            .query_row(&q.replace("select *", "SELECT COUNT(*)"), [], |row| {
                row.get(0)
            })
            .expect("count");
        drop(conn);
        assert_eq!(ink, reference as usize, "reference disagrees on {q}");
    }

    commit_and_close(db);
    assert_integrity_ok(&path);
    cleanup(&path);
}

#[test]
fn a_bounded_interval_is_one_tightened_operator() {
    let path = fixture("rowidtighten");
    let mut db = open_engine(&path);

    let text = plan_text(&mut db, "select * from k where id > 1 and id < 70000");
    assert_eq!(
        text.matches("RowRangeScan").count(),
        1,
        "conjuncts on one key must share a single operator: {text}"
    );
    assert!(
        text.contains("> 1") && text.contains("< 70000"),
        "both bounds must survive tightening: {text}"
    );

    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn the_indexed_column_reports_its_rowid() {
    let path = fixture("rowidcolumn");
    let mut db = open_engine(&path);

    run_ok(&mut db, "create index v_index on k(v)");
    assert_eq!(run_count(&mut db, "select * from k where v = 'c'"), 1);

    commit_and_close(db);
    let conn = Connection::open(&path).expect("reference");
    let (id, v): (i64, String) = conn
        .query_row("SELECT id, v FROM k WHERE id = 500", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("row");
    assert_eq!((id, v.as_str()), (500, "c"));
    drop(conn);
    assert_integrity_ok(&path);
    cleanup(&path);
}
