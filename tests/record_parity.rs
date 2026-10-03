#[path = "common.rs"]
mod common;

use common::*;
use inkdb::Master;
use inkdb::record::Record;
use inkdb::storage::btree::kind::HasPayload;
use inkdb::storage::btree::{BTreeCursor, TableLeaf};
use inkdb::storage::page::PageRef as BTreePageRef;

#[test]
fn record_accessor_agrees_with_the_page_decoder_on_rows() {
    let path = db_path("recordparity");
    build_users(&path, 4096, 500);
    let mut db = open_engine(&path);
    let master = Master::new(db.pager()).expect("master");
    let root_page = master.tables().get("users").expect("users table").root_page();

    let mut cursor = BTreeCursor::new(root_page);
    cursor.first(db.pager()).expect("first");
    let mut checked = 0usize;
    loop {
        let Some(cell) = cursor.current::<TableLeaf>(db.pager()).expect("cell") else {
            break;
        };
        assert!(
            cell.overflow_page().is_none(),
            "fixture rows must be inline for this comparison"
        );
        let (page_no, _) = cursor.last_visited_entry_unchecked();
        let guard = db.pager().get(page_no).expect("page");
        let page = BTreePageRef::new(
            page_no,
            db.pager().page_size(),
            db.pager().usable_size(),
            db.pager().header_len(),
            guard.bytes(),
        )
        .expect("page ref");
        let payload = &page.bytes()[cell.payload_range().clone()];

        let expected = cursor
            .current_record::<TableLeaf>(db.pager())
            .expect("page decode")
            .expect("row");
        let record = Record::new(payload).expect("parse");
        assert_eq!(record.len(), expected.len(), "row {checked}");
        assert_eq!(
            record.to_values().expect("decode"),
            expected,
            "row {checked}"
        );
        assert_eq!(
            record.to_values_owned().expect("decode"),
            expected
                .iter()
                .map(|value| value.to_owned_static())
                .collect::<Vec<_>>(),
            "row {checked}"
        );
        checked += 1;
        cursor.next(db.pager()).expect("next");
    }
    assert_eq!(checked, 500);
    commit_and_close(db);
    cleanup(&path);
}
