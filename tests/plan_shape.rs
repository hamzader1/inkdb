#[path = "common.rs"]
mod common;

use common::*;
use inkdb::Master;
use inkdb::backend::analyzer::Analyze;
use inkdb::backend::planner::plan::Plan;
use inkdb::backend::planner::prepared_plan::PreparedPlan;
use inkdb::db::Database;
use inkdb::sql::lexer::Lexer;
use inkdb::sql::parser::Parser;
use inkdb::vfs::disk::DiskVfs;
use std::rc::Rc;

fn plan_of(db: &mut Database<DiskVfs>, q: &str) -> PreparedPlan<DiskVfs> {
    let query: Rc<str> = Rc::from(q);
    let lexer = Lexer::tokenize(&query).expect("lex");
    let parsed = Parser::parse(Rc::clone(&query), lexer).expect("parse");
    let master = Master::new(&mut db.pager).expect("master");
    let resolved = Analyze::new(&master).analyze(parsed).expect("analyze");
    Plan::create_plan(resolved, &mut db.pager, &master).expect("plan")
}

fn nodes<'a>(plan: &'a Plan<DiskVfs>, out: &mut Vec<&'a Plan<DiskVfs>>) {
    out.push(plan);
    for child in plan.children() {
        nodes(child, out);
    }
}

fn check_shape(db: &mut Database<DiskVfs>, q: &str, has_where: bool) {
    let prepared = plan_of(db, q);
    let mut all = Vec::new();
    nodes(&prepared.parent, &mut all);

    let mut filters = 0;
    let mut index_paths = 0;
    let mut scans = 0;
    let mut pushed = 0;
    for node in &all {
        match node {
            Plan::Filter(filter) => {
                filters += 1;
                assert!(
                    !matches!(filter.child(), Plan::TableScan(_)),
                    "{q}: Filter directly above TableScan - the predicate runs twice"
                );
            }
            Plan::IndexExactMatch(_) | Plan::IndexRangeScan(_) => index_paths += 1,
            Plan::TableScan(scan) => {
                scans += 1;
                if scan.pushed_predicate().is_some() {
                    pushed += 1;
                }
            }
            _ => {}
        }
    }

    assert!(
        filters >= index_paths,
        "{q}: {index_paths} index access path(s) but only {filters} Filter(s): rows would leak"
    );
    if has_where && index_paths == 0 {
        assert_eq!(scans, pushed, "{q}: the WHERE must live in the scan");
    }
}

#[test]
fn a_predicate_is_evaluated_exactly_once() {
    let path = db_path("planshape");
    build_users(&path, 4096, 400);
    let mut db = open_engine(&path);

    check_shape(&mut db, "select * from users where age = 21", true);
    check_shape(&mut db, "select count(*) from users where age = 21", true);
    check_shape(&mut db, "delete from users where age = 21", true);
    check_shape(&mut db, "update users set age = 30 where age = 21", true);
    check_shape(&mut db, "select * from users", false);

    let prepared = plan_of(&mut db, "select * from users where age = 21");
    let label = prepared.parent.node_label(&prepared.arena);
    assert!(
        label.contains("filter:"),
        "the pushed predicate must be visible in EXPLAIN: {label}"
    );

    run_ok(&mut db, "create index age_index on users(age)");

    let prepared = plan_of(&mut db, "select * from users where age = 21");
    let Plan::Filter(filter) = &prepared.parent else {
        panic!("with an index the plan must keep its Filter");
    };
    assert!(matches!(filter.child(), Plan::IndexExactMatch(_)));

    check_shape(&mut db, "select * from users where age = 21", true);
    check_shape(&mut db, "select * from users where age > 21", true);
    check_shape(&mut db, "select * from users where age < 21", true);
    check_shape(&mut db, "select count(*) from users where age = 21", true);
    check_shape(&mut db, "delete from users where age = 21", true);
    check_shape(&mut db, "update users set age = 30 where age = 21", true);
    check_shape(
        &mut db,
        "select * from users where age = 21 or age = 22",
        true,
    );
    check_shape(&mut db, "select * from users where age + 1 = 22", true);

    commit_and_close(db);
    cleanup(&path);
}
