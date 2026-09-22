//! Shared harness for engine-level integration tests.
//!
//! Fixtures are built at test time with real SQLite (rusqlite, bundled),
//! the engine under test mutates them, and real SQLite verifies the result
//! (`PRAGMA integrity_check` plus independent counts). Nothing is checked
//! in: every fixture is generated, used, and deleted by the test run.

#![allow(warnings)]
use inkdb::SqliteMaster;
use inkdb::backend::analyze::Analyze;
use inkdb::backend::planner::plan::Plan;
use inkdb::db::Database;
use inkdb::sql::lexer::Lexer;
use inkdb::sql::parser::Parser;
use inkdb::vfs::disk::DiskVfs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Deterministic RNG (LCG). No extra crates, same sequence everywhere.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

pub fn db_path(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("inkdb-eng-{}-{}-{}.db", std::process::id(), tag, n))
}

pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    let journal = path.with_extension("db-journal");
    let _ = std::fs::remove_file(journal);
}

/// Build a fresh `users` fixture with real SQLite.
///
/// Layout mirrors the historical manual fixture: `n` rows, ages spread
/// over 40 groups (`20 + x % 40`), salaries and positions cycling. Small
/// page sizes give deep trees from few rows, which is what stresses
/// splits, merges, and rebalancing.
pub fn build_users(path: &Path, page_size: u32, n: u64) {
    let _ = std::fs::remove_file(path);
    let conn = rusqlite::Connection::open(path).expect("open fixture");
    conn.execute_batch(&format!(
        "PRAGMA page_size={page_size}; PRAGMA journal_mode=DELETE;"
    ))
    .expect("pragmas");
    conn.execute_batch("VACUUM;").expect("vacuum");
    conn.execute_batch(
        "CREATE TABLE users (name VARCHAR(100), age INT, salary Double, position VARCHAR(100));",
    )
    .expect("schema");
    conn.execute_batch("BEGIN;").expect("begin");
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO users VALUES ('User-' || printf('%06d', ?1), 20 + (?1 % 40), \
                 30000.0 + ((?1 * 13) % 60000), 'Position-' || (?1 % 50))",
            )
            .expect("prepare");
        for x in 1..=n {
            stmt.execute([x as i64]).expect("insert");
        }
    }
    conn.execute_batch("COMMIT;").expect("commit fixture");
}

/// Open the fixture with the engine and start a write transaction.
pub fn open_engine(path: &Path) -> Database<DiskVfs> {
    let mut db = Database::new(path).expect("engine open");
    db.pager.start_transaction();
    db
}

fn ensure_txn(db: &mut Database<DiskVfs>) {
    if !db.pager.in_transaction() {
        db.pager.start_transaction();
    }
}

/// Run any statement, return the number of yielded rows. Panics on error
/// with the query attached, so failures point at the statement.
pub fn run_count(db: &mut Database<DiskVfs>, q: &str) -> usize {
    ensure_txn(db);
    let query = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let query: Rc<str> = Rc::from(query.as_str());
    let lexer = Lexer::tokenize(&query).unwrap_or_else(|e| panic!("lex {q:?}: {e}"));
    let parsed =
        Parser::parse(Rc::clone(&query), lexer).unwrap_or_else(|e| panic!("parse {q:?}: {e}"));
    let master = SqliteMaster::new(&mut db.pager).expect("master");
    let resolved =
        Analyze::analyze(parsed, &master).unwrap_or_else(|e| panic!("analyze {q:?}: {e}"));
    let mut plan = Plan::create_plan(resolved, &mut db.pager, &master)
        .unwrap_or_else(|e| panic!("plan {q:?}: {e}"));
    let mut n = 0;
    loop {
        println!("query: {}", query);
        match plan.next(&mut db.pager) {
            Ok(Some(_)) => n += 1,
            Ok(None) => break,
            Err(e) => panic!("exec {q:?}: {e}"),
        }
    }
    n
}

/// Run a statement expected to yield no rows (DDL, INSERT, DELETE).
pub fn run_ok(db: &mut Database<DiskVfs>, q: &str) {
    let n = run_count(db, q);
    assert_eq!(n, 0, "statement yielded rows: {q:?}");
}

/// Run a statement that must fail (constraint violations). Returns the error.
pub fn run_err(db: &mut Database<DiskVfs>, q: &str) -> String {
    ensure_txn(db);
    let query = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let query: Rc<str> = Rc::from(query.as_str());
    let lexer = Lexer::tokenize(&query).expect("lex");
    let parsed = Parser::parse(Rc::clone(&query), lexer).expect("parse");
    let master = SqliteMaster::new(&mut db.pager).expect("master");
    let resolved = Analyze::analyze(parsed, &master).expect("analyze");
    let mut plan = Plan::create_plan(resolved, &mut db.pager, &master).expect("plan");
    loop {
        match plan.next(&mut db.pager) {
            Ok(Some(_)) => {}
            Ok(None) => panic!("expected error, statement succeeded: {q:?}"),
            Err(e) => return format!("{e}"),
        }
    }
}

/// Commit engine writes so real SQLite can see them, then close.
pub fn commit_and_close(db: Database<DiskVfs>) {
    let mut db = db;
    if db.pager.in_transaction() {
        db.pager.commit().expect("commit");
    }
}

/// Independent row count straight from SQLite, bypassing the engine.
pub fn sqlite_count(path: &Path, where_clause: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("sqlite open");
    let sql = if where_clause.is_empty() {
        "SELECT COUNT(*) FROM users".to_string()
    } else {
        format!("SELECT COUNT(*) FROM users WHERE {where_clause}")
    };
    conn.query_row(&sql, [], |r| r.get(0)).expect("count")
}

/// The file the engine wrote must be a valid SQLite database.
pub fn assert_integrity_ok(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("sqlite open");
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity_check");
    assert_eq!(
        verdict,
        "ok",
        "integrity_check failed for {}",
        path.display()
    );
}
