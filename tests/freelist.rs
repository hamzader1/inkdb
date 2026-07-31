use inkdb::SqliteDatabase;
use std::fs;

fn mutated_fixture(name: &str, mutate: impl FnOnce(&mut [u8])) -> std::path::PathBuf {
    let mut bytes = fs::read("tests/fixtures/freelist.db").unwrap();
    mutate(&mut bytes);

    let path = std::env::temp_dir().join(format!("inkdb-{name}-{}.db", std::process::id()));

    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn valid_freelist_fixture_parses() {
    let mut db = SqliteDatabase::new("tests/fixtures/freelist.db").unwrap();

    assert!(db.freelist().is_ok());
}

#[test]
fn zero_head_with_nonzero_count_returns_error_not_panic() {
    let path = mutated_fixture("zero-head", |bytes| {
        bytes[32..36].copy_from_slice(&0u32.to_be_bytes());
        bytes[36..40].copy_from_slice(&1u32.to_be_bytes());
    });

    let result = std::panic::catch_unwind(|| {
        let mut db = SqliteDatabase::new(&path).unwrap();
        db.freelist()
    });

    assert!(matches!(result, Ok(Err(_))));
}

#[test]
fn nonzero_head_with_zero_count_returns_error_not_panic() {
    let path = mutated_fixture("zero-count", |bytes| {
        bytes[32..36].copy_from_slice(&2u32.to_be_bytes());
        bytes[36..40].copy_from_slice(&0u32.to_be_bytes());
    });

    let result = std::panic::catch_unwind(|| {
        let mut db = SqliteDatabase::new(&path).unwrap();
        db.freelist()
    });

    assert!(matches!(result, Ok(Err(_))));
}

#[test]
fn out_of_range_trunk_page_returns_error_not_panic() {
    let path = mutated_fixture("bad-trunk", |bytes| {
        bytes[32..36].copy_from_slice(&999u32.to_be_bytes());
        bytes[36..40].copy_from_slice(&1u32.to_be_bytes());
    });

    let result = std::panic::catch_unwind(|| {
        let mut db = SqliteDatabase::new(&path).unwrap();
        db.freelist()
    });

    assert!(matches!(result, Ok(Err(_))));
}
