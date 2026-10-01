#[path = "common.rs"]
mod common;
use common::*;
use inkdb::Master;
use inkdb::backend::analyzer::Analyze;
use inkdb::backend::planner::plan::Plan;
use rusqlite::Connection;
use std::rc::Rc;

#[test]
fn probe() {
    let path = db_path("zzprobe");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE t1 (name TEXT, age INT); INSERT INTO t1 VALUES ('a',1),('b',2);").unwrap();
    drop(c);
    let mut db = open_engine(&path);
    run_ok(&mut db, "create unique index alpha on t1(name)");
    let q: Rc<str> = Rc::from("select * from t1 where name = 'a'");
    let lex = inkdb::sql::lexer::Lexer::tokenize(&q).unwrap();
    let ast = inkdb::sql::parser::Parser::parse(Rc::clone(&q), lex).unwrap();
    let master = Master::new(&mut db.pager).unwrap();
    eprintln!("INDEXES: {:?}", master.indexes.keys().collect::<Vec<_>>());
    for (k, v) in master.indexes.iter() {
        eprintln!("  {k}: table={} cols={:?} unique={} root={}", v.table, v.columns, v.unique, v.root_page);
    }
    let resolved = Analyze::new(&master).analyze(ast).unwrap();
    let p = Plan::create_plan(resolved, &mut db.pager, &master).unwrap();
    fn walk(plan: &Plan<inkdb::vfs::disk::DiskVfs>, arena: &inkdb::sql::parser::ExprArena, depth: usize) {
        eprintln!("{}{}", "    ".repeat(depth), plan.node_label(arena));
        for child in plan.children() {
            walk(child, arena, depth + 1);
        }
    }
    eprintln!("PLAN TREE:");
    walk(&p.parent, &p.arena, 0);
    commit_and_close(db);
    cleanup(&path);
}
