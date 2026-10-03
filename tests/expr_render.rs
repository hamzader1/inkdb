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

#[test]
fn explain_names_columns_instead_of_arena_slots() {
    let path = db_path("exprrender");
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch("CREATE TABLE t1 (name TEXT, age INT);")
        .expect("table");
    drop(conn);
    let mut db = open_engine(&path);

    let text = plan_text(&mut db, "select * from t1 where age = 20");
    assert!(
        text.contains("(age = 20)"),
        "the pushed predicate should read as SQL: {text}"
    );
    assert!(
        !text.contains("column["),
        "no arena slot index may leak into EXPLAIN: {text}"
    );

    let text = plan_text(&mut db, "select count(*) from t1 where age + 1 = 22");
    assert!(text.contains("count(*)"), "{text}");
    assert!(text.contains("((age + 1) = 22)"), "{text}");

    run_ok(&mut db, "create index age_index on t1(age)");
    let text = plan_text(&mut db, "select * from t1 where age > 20");
    assert!(
        text.contains("Filter [(age > 20)]"),
        "the filter above an index path should name the column: {text}"
    );

    commit_and_close(db);
    cleanup(&path);
}

#[test]
fn unique_violation_names_the_table_and_column() {
    let path = db_path("exprunique");
    let conn = Connection::open(&path).expect("fixture");
    conn.execute_batch(
        "CREATE TABLE t1 (name TEXT, age INT);\n         INSERT INTO t1 VALUES ('alpha', 1), ('alpha', 2);",
    )
    .expect("rows");
    drop(conn);
    let mut db = open_engine(&path);

    let message = run_err(&mut db, "create unique index alpha on t1(name)");
    assert!(
        message.contains("t1.name"),
        "the violation should name table and column: {message}"
    );

    cleanup(&path);
}
