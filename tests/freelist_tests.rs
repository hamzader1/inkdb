//! Tests for `inkdb::format::freelist`.
//!
//! `SqliteDatabase::freelist()` requires a live `SqliteDatabase<S>`, and
//! (per the note in overflow_tests.rs) the only public constructor is
//! `SqliteDatabase::new()`, hard-coded to `DiskFile`. So these tests write
//! real temp files.
//!
//! NOTE: `FreeList`'s fields (`trunk_pages`, `leaf_pages`) are private with
//! no accessors, and `SqliteDatabase::freelist()` returns
//! `Result<Option<FreeList>, DbError>` rather than exposing the list
//! contents. That means from outside the crate we can only assert on
//! Ok(Some(_)) / Ok(None) / Err(_), not on the actual trunk/leaf page
//! numbers collected. Tests below are written to that constraint.

mod common;

use common::{build_db_image, build_freelist_trunk_page, set_freelist};
use inkdb::SqliteDatabase;
use std::io::Write;

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("inkdb_freelist_test_{}_{}.db", std::process::id(), name));
    p
}

fn write_db_file(path: &std::path::Path, image: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(image).unwrap();
    f.sync_all().unwrap();
}

struct TempDb(std::path::PathBuf);
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn no_freelist_returns_none() {
    let page_size = 512usize;
    let image = build_db_image(page_size, 2, |h| {
        set_freelist(h, 0, 0); // no trunk page, zero count
    });
    let path = temp_db_path("none");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist().unwrap();
    assert!(result.is_none());
}

#[test]
fn trunk_page_nonzero_but_count_zero_is_corrupt() {
    let page_size = 512usize;
    let image = build_db_image(page_size, 3, |h| {
        set_freelist(h, 2, 0); // trunk set but count says zero
    });
    let path = temp_db_path("trunk_but_no_count");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist();
    assert!(result.is_err());
}

#[test]
fn count_nonzero_but_trunk_zero_is_corrupt() {
    let page_size = 512usize;
    let image = build_db_image(page_size, 2, |h| {
        set_freelist(h, 0, 5); // count says 5 but no trunk page
    });
    let path = temp_db_path("count_but_no_trunk");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist();
    assert!(result.is_err());
}

#[test]
fn single_trunk_page_no_leaves() {
    let page_size = 512usize;
    let page_count = 2u32;
    let mut image = build_db_image(page_size, page_count, |h| {
        set_freelist(h, 2, 1); // trunk = page 2, total freelist pages = 1
    });

    let trunk = build_freelist_trunk_page(0, &[], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk);

    let path = temp_db_path("single_trunk");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist().unwrap();
    assert!(result.is_some());
}

#[test]
fn trunk_with_leaf_pages() {
    let page_size = 512usize;
    let page_count = 4u32; // page 2 = trunk, pages 3,4 = leaves
    let mut image = build_db_image(page_size, page_count, |h| {
        set_freelist(h, 2, 3); // 1 trunk + 2 leaves = 3 total
    });

    let trunk = build_freelist_trunk_page(0, &[3, 4], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk);

    let path = temp_db_path("trunk_with_leaves");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist().unwrap();
    assert!(result.is_some());
}

#[test]
fn multiple_trunk_pages_chain() {
    let page_size = 512usize;
    let page_count = 3u32; // page 2 = trunk1 -> page 3 = trunk2 (no leaves)
    let mut image = build_db_image(page_size, page_count, |h| {
        set_freelist(h, 2, 2); // 2 trunks, 0 leaves
    });

    let trunk1 = build_freelist_trunk_page(3, &[], page_size);
    let trunk2 = build_freelist_trunk_page(0, &[], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk1);
    image[page_size * 2..page_size * 3].copy_from_slice(&trunk2);

    let path = temp_db_path("multi_trunk");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist().unwrap();
    assert!(result.is_some());
}

#[test]
fn freelist_page_count_mismatch_is_corrupt() {
    let page_size = 512usize;
    let page_count = 3u32;
    let mut image = build_db_image(page_size, page_count, |h| {
        // Header claims 5 total freelist pages but only 1 trunk with 0
        // leaves actually exists in the chain.
        set_freelist(h, 2, 5);
    });

    let trunk = build_freelist_trunk_page(0, &[], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk);

    let path = temp_db_path("count_mismatch");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist();
    assert!(result.is_err());
}

#[test]
fn leaf_page_number_zero_in_trunk_is_rejected() {
    let page_size = 512usize;
    let page_count = 3u32;
    let mut image = build_db_image(page_size, page_count, |h| {
        set_freelist(h, 2, 2);
    });

    // Leaf page 0 is invalid (page numbers are 1-indexed).
    let trunk = build_freelist_trunk_page(0, &[0], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk);

    let path = temp_db_path("leaf_zero");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist();
    assert!(result.is_err());
}

#[test]
fn leaf_page_number_out_of_database_range_is_rejected() {
    let page_size = 512usize;
    let page_count = 3u32;
    let mut image = build_db_image(page_size, page_count, |h| {
        set_freelist(h, 2, 2);
    });

    // Leaf page 999 doesn't exist in a 3-page database.
    let trunk = build_freelist_trunk_page(0, &[999], page_size);
    image[page_size..page_size * 2].copy_from_slice(&trunk);

    let path = temp_db_path("leaf_out_of_range");
    write_db_file(&path, &image);
    let _guard = TempDb(path.clone());

    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.freelist();
    assert!(result.is_err());
}
