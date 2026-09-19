use inkdb::pager::pager::Pager;
use inkdb::vfs::SqliteOptions;
use inkdb::vfs::disk::{DiskFile, DiskVfs};
use inkdb::vfs::file::SqliteFile;
use inkdb::vfs::Vfs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const PS: usize = 512;

static SEQ: AtomicU64 = AtomicU64::new(0);

fn db_path(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("inkdb-pager-{}-{}-{}.db", std::process::id(), tag, n))
}

fn test_pager(tag: &str, cache: usize, npages: usize) -> (Pager<DiskFile>, PathBuf) {
    let path = db_path(tag);
    let mut vfs = DiskVfs;
    let source: DiskFile = vfs.open(&path, SqliteOptions::all()).unwrap();
    source.set_len(PS * npages).unwrap();
    let pager = Pager::with_cache(source, PS, PS, npages, 0, 0, cache).unwrap();
    (pager, path)
}

fn journal_path(db: &PathBuf) -> PathBuf {
    let name = db.file_name().unwrap().to_str().unwrap().to_owned();
    db.parent().unwrap().join(format!("{}-journal", name))
}

fn cleanup(db: &PathBuf) {
    let _ = std::fs::remove_file(db);
    let _ = std::fs::remove_file(journal_path(db));
}

fn read_page(db: &PathBuf, page_no: u32) -> Vec<u8> {
    let mut vfs = DiskVfs;
    let source: DiskFile = vfs.open(db, SqliteOptions::default()).unwrap();
    let mut buf = vec![0u8; PS];
    source
        .read_exact_at(((page_no as usize - 1) * PS) as u64, &mut buf)
        .unwrap();
    buf
}

#[test]
fn get_returns_zeros_and_validates() {
    let (mut pager, path) = test_pager("get", 4, 4);
    let g = pager.get(1).unwrap();
    assert_eq!(g.bytes_as_ref().len(), PS);
    assert!(g.bytes_as_ref().iter().all(|b| *b == 0));
    drop(g);
    assert!(pager.get(0).is_err());
    assert!(pager.get(5).is_err());
    cleanup(&path);
}

#[test]
fn commit_persists_writes() {
    let (mut pager, path) = test_pager("commit", 4, 4);
    assert!(pager.start_transaction());
    {
        let mut g = pager.get_mut(2).unwrap();
        g.bytes_as_mut_unchecked().fill(0xAB);
    }
    pager.commit().unwrap();
    assert!(!pager.in_transaction());
    drop(pager);
    let raw = read_page(&path, 2);
    assert!(raw.iter().all(|b| *b == 0xAB));
    let raw1 = read_page(&path, 1);
    assert!(raw1.iter().all(|b| *b == 0));
    cleanup(&path);
}

#[test]
fn rollback_restores_previous_content() {
    let (mut pager, path) = test_pager("rollback", 8, 4);
    pager.start_transaction();
    {
        let mut g = pager.get_mut(2).unwrap();
        g.bytes_as_mut_unchecked().fill(0x11);
    }
    pager.commit().unwrap();
    pager.start_transaction();
    {
        let mut g = pager.get_mut(2).unwrap();
        g.bytes_as_mut_unchecked().fill(0x22);
    }
    pager.rollback().unwrap();
    assert!(!pager.in_transaction());
    let g = pager.get(2).unwrap();
    assert!(g.bytes_as_ref().iter().all(|b| *b == 0x11));
    drop(g);
    drop(pager);
    let raw = read_page(&path, 2);
    assert!(raw.iter().all(|b| *b == 0x11));
    cleanup(&path);
}

#[test]
fn rollback_truncates_grown_file() {
    let (mut pager, path) = test_pager("truncate", 8, 4);
    let len_before = std::fs::metadata(&path).unwrap().len();
    pager.start_transaction();
    let grown = pager.allocate_new_page().unwrap();
    assert_eq!(grown, 5);
    pager.rollback().unwrap();
    let len_after = std::fs::metadata(&path).unwrap().len();
    assert_eq!(len_before, len_after);
    assert!(pager.get(5).is_err());
    cleanup(&path);
}

#[test]
fn commit_with_eviction_flushes_all_dirty() {
    let (mut pager, path) = test_pager("evict", 2, 4);
    pager.start_transaction();
    for (page, byte) in [(1u32, 0xA1u8), (2, 0xA2), (3, 0xA3), (4, 0xA4)] {
        let mut g = pager.get_mut(page).unwrap();
        g.bytes_as_mut_unchecked().fill(byte);
    }
    pager.commit().unwrap();
    drop(pager);
    for (page, byte) in [(1u32, 0xA1u8), (2, 0xA2), (3, 0xA3), (4, 0xA4)] {
        let raw = read_page(&path, page);
        assert!(raw.iter().all(|b| *b == byte), "page {} mismatch", page);
    }
    cleanup(&path);
}
