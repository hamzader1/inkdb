#[path = "common.rs"]
mod common;

use common::*;
use rusqlite::Connection;

#[test]
fn a_payload_bigger_than_a_page_is_stored_and_read_back() {
    let path = db_path("ovf");
    {
        let conn = Connection::open(&path).expect("fixture");
        conn.execute_batch("CREATE TABLE t (a TEXT, b INTEGER);")
            .expect("fixture");
    }

    let mut db = open_engine(&path);
    for (size, tag) in [(3000usize, "a"), (9000, "b"), (40000, "c")] {
        let big = tag.repeat(size / tag.len());
        run_ok(&mut db, &format!("insert into t values ('{big}', {size})"));
    }
    commit_and_close(db);
    assert_integrity_ok(&path);

    let conn = Connection::open(&path).expect("reference");
    for (size, tag) in [(3000usize, "a"), (9000, "b"), (40000, "c")] {
        let expected = tag.repeat(size / tag.len());
        let got: String = conn
            .query_row("SELECT a FROM t WHERE b = ?", [size as i64], |row| {
                row.get(0)
            })
            .expect("the reference must read the row");
        assert_eq!(got.len(), expected.len(), "size {size}");
        assert_eq!(got, expected, "content for size {size}");
    }
    drop(conn);
    cleanup(&path);
}
