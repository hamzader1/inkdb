use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[path = "common.rs"]
mod common;

use common::*;
use inkdb::Master;
use inkdb::backend::analyzer::Analyze;
use inkdb::backend::executor::RowWrapper;
use inkdb::backend::planner::plan::Plan;
use inkdb::db::Database;
use inkdb::sql::lexer::Lexer;
use inkdb::sql::parser::Parser;
use inkdb::vfs::disk::DiskVfs;
use std::rc::Rc;

struct Outcome {
    rows: usize,
    plan_allocations: usize,
    exec_allocations: usize,
    first_row: String,
}

fn measure(db: &mut Database<DiskVfs>, q: &str) -> Outcome {
    let query: Rc<str> = Rc::from(q);
    let lexer = Lexer::tokenize(&query).expect("lex");
    let parsed = Parser::parse(Rc::clone(&query), lexer).expect("parse");
    let mut master = Master::new(db.pager()).expect("master");
    let resolved = Analyze::new(&master).analyze(parsed).expect("analyze");

    let before_plan = ALLOCATIONS.load(Ordering::Relaxed);
    let mut plan = Plan::create_plan(resolved, db.pager(), &master).expect("plan");
    let after_plan = ALLOCATIONS.load(Ordering::Relaxed);

    let mut rows = 0usize;
    let mut first_row = String::new();
    loop {
        match plan.next(db.pager(), &mut master) {
            Ok(Some(row)) => {
                if rows == 0 {
                    first_row = format!("{}", RowWrapper(row));
                }
                rows += 1;
            }
            Ok(None) => break,
            Err(e) => panic!("exec {q}: {e}"),
        }
    }
    let after_exec = ALLOCATIONS.load(Ordering::Relaxed);
    Outcome {
        rows,
        plan_allocations: after_plan - before_plan,
        exec_allocations: after_exec - after_plan,
        first_row,
    }
}

#[test]
fn allocation_profile_of_a_full_scan() {
    let path = db_path("allocprofile");
    build_users(&path, 4096, 100_000);
    let mut db = open_engine(&path);

    let queries = [
        "select count(*) from users",
        "select count(*) from users where age = 21",
        "select name from users",
        "select * from users",
        "select * from users where age = 21",
    ];
    for query in queries {
        let outcome = measure(&mut db, query);
        let per_row = outcome.exec_allocations as f64 / outcome.rows.max(1) as f64;
        println!(
            "{query:44} rows={:7} plan_allocs={:6} exec_allocs={:9} ({per_row:.2}/row) first={}",
            outcome.rows, outcome.plan_allocations, outcome.exec_allocations, outcome.first_row
        );
    }
    let count = measure(&mut db, "select count(*) from users");
    assert_eq!(count.rows, 1);
    assert_eq!(count.first_row, "100000");
    let filtered = measure(&mut db, "select count(*) from users where age = 21");
    assert_eq!(filtered.rows, 1);
    assert_eq!(filtered.first_row, db_count(&path, "age = 21").to_string());
    assert_eq!(measure(&mut db, "select * from users").rows, 100_000);
    commit_and_close(db);
    cleanup(&path);

    let nulls = db_path("allocnulls");
    let conn = rusqlite::Connection::open(&nulls).expect("null fixture");
    conn.execute_batch(
        "CREATE TABLE t (a INT, b INT); INSERT INTO t VALUES (1, NULL), (NULL, 2), (3, 3);",
    )
    .expect("null rows");
    drop(conn);
    let mut null_db = open_engine(&nulls);
    assert_eq!(
        measure(&mut null_db, "select count(*) from t").first_row,
        "3"
    );
    assert_eq!(
        measure(&mut null_db, "select count(a) from t").first_row,
        "2"
    );
    assert_eq!(
        measure(&mut null_db, "select count(b) from t").first_row,
        "2"
    );
    commit_and_close(null_db);
    cleanup(&nulls);
}
