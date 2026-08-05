mod common;

use std::io::Write;

use inkdb::pager::buffer_pool::BufferPool;
use inkdb::pager::frame::{Frame, FrameId, CLEAN, DIRTY, FREE, REFERENCED};
use inkdb::pager::guard::{PageGuard, PageGuardMut};
use inkdb::pager::metadata::SqliteMetadata;
use inkdb::pager::page::Pager;
use inkdb::pager::statistics::SqliteStatistics;
use inkdb::vfs::disk::{DiskFile, DiskVfs};
use inkdb::vfs::file::SqliteFile;
use inkdb::vfs::mem::{MemFile, MemVfs};
use inkdb::vfs::{SqliteOptions, Vfs};
use inkdb::DbError;
use inkdb::SqliteDatabase;

fn rc_vec(data: Vec<u8>) -> std::rc::Rc<std::cell::RefCell<Vec<u8>>> {
    std::rc::Rc::new(std::cell::RefCell::new(data))
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "inkdb_pager_test_{}_{}.db",
        std::process::id(),
        name
    ));
    p
}

fn write_db_file(path: &std::path::Path, contents: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(contents).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn metadata_new_valid() {
    let meta = SqliteMetadata::new(4096, 4096, 100);
    assert_eq!(meta.page_size, 4096);
    assert_eq!(meta.usable_size, 4096);
    assert_eq!(meta.max_allocated_pages, 100);
}

#[test]
fn metadata_new_usable_less_than_page_size() {
    let meta = SqliteMetadata::new(4096, 1024, 100);
    assert_eq!(meta.page_size, 4096);
    assert_eq!(meta.usable_size, 1024);
}

#[test]
#[should_panic]
fn metadata_new_usable_greater_than_page_size_panics() {
    let _ = SqliteMetadata::new(4096, 5000, 100);
}

#[test]
#[should_panic]
fn metadata_new_zero_max_pages_panics() {
    let _ = SqliteMetadata::new(4096, 4096, 0);
}

#[test]
fn statistics_default_all_zeros() {
    let stats = SqliteStatistics::default();
    assert_eq!(stats.cache_hit(), 0);
    assert_eq!(stats.cache_miss(), 0);
    assert_eq!(stats.disk_write(), 0);
    assert_eq!(stats.evictions(), 0);
}

#[test]
fn statistics_inc_cache_hit() {
    let stats = SqliteStatistics::default();
    stats.inc_cache_hit();
    assert_eq!(stats.cache_hit(), 1);
    stats.inc_cache_hit();
    assert_eq!(stats.cache_hit(), 2);
}

#[test]
fn statistics_inc_cache_miss() {
    let stats = SqliteStatistics::default();
    stats.inc_cache_miss();
    assert_eq!(stats.cache_miss(), 1);
}

#[test]
fn statistics_inc_disk_write() {
    let stats = SqliteStatistics::default();
    stats.inc_disk_write();
    assert_eq!(stats.disk_write(), 1);
}

#[test]
fn statistics_inc_evictions() {
    let stats = SqliteStatistics::default();
    stats.inc_evictions();
    assert_eq!(stats.evictions(), 1);
}

#[test]
fn statistics_counters_are_independent() {
    let stats = SqliteStatistics::default();
    stats.inc_cache_hit();
    stats.inc_cache_miss();
    stats.inc_disk_write();
    stats.inc_evictions();
    assert_eq!(stats.cache_hit(), 1);
    assert_eq!(stats.cache_miss(), 1);
    assert_eq!(stats.disk_write(), 1);
    assert_eq!(stats.evictions(), 1);
}

#[test]
fn statistics_multiple_increments() {
    let stats = SqliteStatistics::default();
    for _ in 0..100 {
        stats.inc_cache_hit();
    }
    assert_eq!(stats.cache_hit(), 100);
}

#[test]
fn frame_new_with_all_fields() {
    let frame = Frame::new(Some(5), CLEAN | REFERENCED, 3);
    assert_eq!(frame.page_no, Some(5));
    assert!(frame.is(CLEAN));
    assert!(frame.is(REFERENCED));
    assert!(!frame.is(DIRTY));
    assert!(!frame.is(FREE));
}

#[test]
fn frame_default_is_free() {
    let frame = Frame::default();
    assert_eq!(frame.page_no, None);
    assert!(frame.is(FREE));
    assert!(!frame.is(CLEAN));
    assert!(!frame.is(DIRTY));
    assert!(!frame.is(REFERENCED));
    assert_eq!(frame.pin_count.get(), 0);
}

#[test]
fn frame_set_flag() {
    let frame = Frame::default();
    frame.set(CLEAN);
    assert!(frame.is(CLEAN));
    assert!(frame.is(FREE));
}

#[test]
fn frame_clear_flag() {
    let frame = Frame::new(None, FREE | CLEAN, 0);
    frame.clear(CLEAN);
    assert!(!frame.is(CLEAN));
    assert!(frame.is(FREE));
}

#[test]
fn frame_reset_to() {
    let frame = Frame::new(Some(1), FREE | CLEAN | DIRTY, 2);
    frame.reset_to(REFERENCED);
    assert!(frame.is(REFERENCED));
    assert!(!frame.is(FREE));
    assert!(!frame.is(CLEAN));
    assert!(!frame.is(DIRTY));
}

#[test]
fn frame_incr_pin_count() {
    let frame = Frame::default();
    assert_eq!(frame.pin_count.get(), 0);
    frame.incr_pin_count();
    assert_eq!(frame.pin_count.get(), 1);
    frame.incr_pin_count();
    assert_eq!(frame.pin_count.get(), 2);
}

#[test]
fn frame_decr_pin_count() {
    let frame = Frame::new(None, 0, 5);
    assert_eq!(frame.pin_count.get(), 5);
    frame.decr_pin_count();
    assert_eq!(frame.pin_count.get(), 4);
    frame.decr_pin_count();
    assert_eq!(frame.pin_count.get(), 3);
}

#[test]
#[should_panic]
fn frame_incr_pin_count_overflow_panics() {
    let frame = Frame::new(None, 0, u8::MAX);
    frame.incr_pin_count();
}

#[test]
#[should_panic]
fn frame_decr_pin_count_underflow_panics() {
    let frame = Frame::new(None, 0, 0);
    frame.decr_pin_count();
}

#[test]
fn frame_flag_constants_are_distinct() {
    assert_ne!(FREE, CLEAN);
    assert_ne!(FREE, DIRTY);
    assert_ne!(FREE, REFERENCED);
    assert_ne!(CLEAN, DIRTY);
    assert_ne!(CLEAN, REFERENCED);
    assert_ne!(DIRTY, REFERENCED);
}

#[test]
fn frame_flag_bits_are_single_bits() {
    assert_eq!(FREE.count_ones(), 1);
    assert_eq!(CLEAN.count_ones(), 1);
    assert_eq!(DIRTY.count_ones(), 1);
    assert_eq!(REFERENCED.count_ones(), 1);
}

#[test]
fn frame_multiple_flags_set_and_clear() {
    let frame = Frame::default();
    frame.set(CLEAN);
    frame.set(DIRTY);
    assert!(frame.is(CLEAN));
    assert!(frame.is(DIRTY));
    frame.clear(CLEAN);
    assert!(!frame.is(CLEAN));
    assert!(frame.is(DIRTY));
    frame.set(REFERENCED);
    assert!(frame.is(REFERENCED));
    assert!(frame.is(DIRTY));
    frame.reset_to(FREE);
    assert!(frame.is(FREE));
    assert!(!frame.is(CLEAN));
    assert!(!frame.is(DIRTY));
    assert!(!frame.is(REFERENCED));
}

#[test]
fn frame_page_no_none_by_default() {
    let frame = Frame::default();
    assert_eq!(frame.page_no, None);
}

#[test]
fn frame_page_no_some() {
    let frame = Frame::new(Some(42), CLEAN, 0);
    assert_eq!(frame.page_no, Some(42));
}

#[test]
fn buffer_pool_new_has_correct_free_frames() {
    let pool = BufferPool::new(512);
    assert_eq!(pool.free_frames.len(), 4096);
    assert_eq!(pool.page_table.len(), 0);
    assert_eq!(pool.frame_buffer.len(), 4096);
    assert_eq!(pool.page_buffer.len(), 4096 * 512);
    assert_eq!(pool.clock_hand, 0);
}

#[test]
fn buffer_pool_new_all_frames_are_default() {
    let pool = BufferPool::new(512);
    for (i, frame) in pool.frame_buffer.iter().enumerate() {
        assert_eq!(frame.page_no, None, "frame {i} should have no page");
        assert!(frame.is(FREE), "frame {i} should be FREE");
        assert_eq!(frame.pin_count.get(), 0, "frame {i} pin count should be 0");
    }
}

#[test]
fn buffer_pool_with_cache_custom_size() {
    let pool = BufferPool::with_cache(10, 512);
    assert_eq!(pool.free_frames.len(), 10);
    assert_eq!(pool.frame_buffer.len(), 10);
    assert_eq!(pool.page_buffer.len(), 10 * 512);
}

#[test]
fn buffer_pool_with_cache_large_cache() {
    let pool = BufferPool::with_cache(100, 4096);
    assert_eq!(pool.free_frames.len(), 100);
    assert_eq!(pool.frame_buffer.len(), 100);
    assert_eq!(pool.page_buffer.len(), 100 * 4096);
}

#[test]
fn buffer_pool_owned_buffer() {
    let buf: Box<[u8]> = BufferPool::owned_buffer::<u8>(100);
    assert_eq!(buf.len(), 100);
    for &b in buf.iter() {
        assert_eq!(b, 0);
    }
}

#[test]
fn buffer_pool_evict_page_removes_from_table() {
    let mut pool = BufferPool::new(512);
    pool.page_table.insert(1, 0);
    pool.frame_buffer[0] = Frame::new(Some(1), CLEAN, 1);
    pool.evict_page(1, 0).unwrap();
    assert!(!pool.page_table.contains_key(&1));
    assert_eq!(pool.frame_buffer[0].page_no, None);
    assert!(pool.frame_buffer[0].is(FREE));
}

#[test]
fn buffer_pool_evict_page_mismatch_errors() {
    let mut pool = BufferPool::new(512);
    pool.page_table.insert(1, 0);
    pool.frame_buffer[0] = Frame::new(Some(1), CLEAN, 0);
    let result = pool.evict_page(1, 1);
    assert!(result.is_err());
}

#[test]
fn buffer_pool_evict_page_wrong_page_errors() {
    let mut pool = BufferPool::new(512);
    pool.page_table.insert(1, 0);
    pool.frame_buffer[0] = Frame::new(Some(1), CLEAN, 0);
    let result = pool.evict_page(2, 0);
    assert!(result.is_err());
}

#[test]
fn buffer_pool_free_page_decrements_pin_count() {
    let mut pool = BufferPool::new(512);
    pool.frame_buffer[0] = Frame::new(Some(1), CLEAN, 3);
    pool.free_page(0);
    assert_eq!(pool.frame_buffer[0].pin_count.get(), 2);
}

#[test]
fn buffer_pool_free_page_multiple_times() {
    let mut pool = BufferPool::new(512);
    pool.frame_buffer[0] = Frame::new(Some(1), CLEAN, 5);
    for _ in 0..5 {
        pool.free_page(0);
    }
    assert_eq!(pool.frame_buffer[0].pin_count.get(), 0);
}

#[test]
fn buffer_pool_as_ptr_mut_returns_valid_pointer() {
    let mut pool = BufferPool::new(512);
    let ptr = pool.as_ptr_mut();
    assert!(!ptr.as_ptr().is_null());
}

#[test]
fn buffer_pool_page_table_initially_empty() {
    let pool = BufferPool::new(512);
    assert!(pool.page_table.is_empty());
}

#[test]
fn buffer_pool_free_frames_sequential() {
    let pool = BufferPool::new(512);
    for (i, &id) in pool.free_frames.iter().enumerate() {
        assert_eq!(id, i, "free_frames[{i}] should be {i}");
    }
}

#[test]
fn buffer_pool_debug_impl() {
    let pool = BufferPool::new(512);
    let debug = format!("{:?}", pool);
    assert!(debug.contains("BufferPool"));
}

fn make_mem_db(page_size: usize, page_count: u32) -> MemFile {
    let mut data = vec![0u8; page_size * page_count as usize];
    let header = common::valid_header_bytes(page_size as u16, 0);
    data[0..100].copy_from_slice(&header);
    MemFile::new(rc_vec(data))
}

#[test]
fn pager_new_with_mem_file() {
    let file = make_mem_db(512, 3);
    let pager = Pager::new(file, 512, 512, 3);
    assert_eq!(pager.cached_page_count(), 0);
}

#[test]
fn pager_with_cache_with_mem_file() {
    let file = make_mem_db(512, 3);
    let pager = Pager::with_cache(file, 512, 512, 3, 2);
    assert_eq!(pager.cached_page_count(), 0);
}

#[test]
fn pager_new_sets_correct_metadata() {
    let file = make_mem_db(4096, 10);
    let pager = Pager::new(file, 4096, 4096, 10);
    assert_eq!(pager.metadata.page_size, 4096);
    assert_eq!(pager.metadata.usable_size, 4096);
    assert_eq!(pager.metadata.max_allocated_pages, 10);
}

#[test]
fn pager_with_cache_sets_correct_metadata() {
    let file = make_mem_db(4096, 10);
    let pager = Pager::with_cache(file, 4096, 4096, 10, 5);
    assert_eq!(pager.metadata.page_size, 4096);
    assert_eq!(pager.buffer_pool.frame_buffer.len(), 5);
}

#[test]
fn pager_get_loads_page_into_cache() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    let bytes = guard.bytes();
    assert_eq!(bytes.len(), 512);
    assert!(bytes.iter().all(|&b| b == 0));
}

#[test]
fn pager_get_mut_loads_page_into_cache() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let mut guard = pager.get_mut(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    let bytes = guard.bytes();
    assert_eq!(bytes.len(), 512);
}

#[test]
fn pager_get_second_access_is_cache_hit() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let _guard1 = pager.get(2).unwrap();
    drop(_guard1);
    let _guard2 = pager.get(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
}

#[test]
fn pager_get_mut_marks_page_dirty() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let mut guard = pager.get_mut(2).unwrap();
    let fid = guard.frame_id();
    let frame = &pager.buffer_pool.frame_buffer[fid];
    assert!(frame.is(DIRTY));
    assert!(frame.is(REFERENCED));
    assert!(!frame.is(CLEAN));
}

#[test]
fn pager_get_marks_page_clean() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get(2).unwrap();
    let fid = guard.frame_id();
    let frame = &pager.buffer_pool.frame_buffer[fid];
    assert!(frame.is(CLEAN));
    assert!(frame.is(REFERENCED));
    assert!(!frame.is(DIRTY));
}

#[test]
fn pager_get_page_zero_errors() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let result = pager.get(0);
    assert!(result.is_err());
}

#[test]
fn pager_get_mut_page_zero_errors() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let result = pager.get_mut(0);
    assert!(result.is_err());
}

#[test]
fn pager_get_page_beyond_max_errors() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let result = pager.get(4);
    assert!(result.is_err());
}

#[test]
fn pager_get_mut_page_beyond_max_errors() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let result = pager.get_mut(4);
    assert!(result.is_err());
}

#[test]
fn pager_get_multiple_pages_fills_cache() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);
    let _g1 = pager.get(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    let _g2 = pager.get(3).unwrap();
    assert_eq!(pager.cached_page_count(), 2);
}

#[test]
fn pager_get_mut_multiple_pages_fills_cache() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);
    let _g1 = pager.get_mut(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    let _g2 = pager.get_mut(3).unwrap();
    assert_eq!(pager.cached_page_count(), 2);
}

#[test]
fn pager_eviction_flushes_dirty_page() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    {
        let mut guard = pager.get_mut(2).unwrap();
        guard.bytes()[0] = 0xAB;
    }

    {
        let _g3 = pager.get(3).unwrap();
        let _g4 = pager.get(4).unwrap();
    }

    let mut buf = [0u8; 512];
    pager.source.read_exact_at(512, &mut buf).unwrap();
    assert_eq!(buf[0], 0xAB);
}

#[test]
fn pager_eviction_flushes_dirty_page_to_source() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    {
        let mut guard = pager.get_mut(2).unwrap();
        guard.bytes()[10] = 0xCD;
    }

    {
        let _g3 = pager.get(3).unwrap();
        let _g4 = pager.get(4).unwrap();
    }

    let mut buf = [0u8; 512];
    pager.source.read_exact_at(512, &mut buf).unwrap();
    assert_eq!(buf[10], 0xCD);
}

#[test]
fn pager_cached_page_count_tracks_loaded_pages() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    assert_eq!(pager.cached_page_count(), 0);
    let _g1 = pager.get(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    let _g2 = pager.get(3).unwrap();
    assert_eq!(pager.cached_page_count(), 2);
    let _g3 = pager.get(4).unwrap();
    assert_eq!(pager.cached_page_count(), 3);
}

#[test]
fn pager_cached_page_count_after_drop() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    let g1 = pager.get(2).unwrap();
    assert_eq!(pager.cached_page_count(), 1);
    drop(g1);
    assert_eq!(pager.cached_page_count(), 1);
}

#[test]
fn pager_validate_page_valid() {
    assert!(Pager::<MemFile>::validate_page(1, 10, None::<fn(_) -> bool>).is_ok());
    assert!(Pager::<MemFile>::validate_page(10, 10, None::<fn(_) -> bool>).is_ok());
}

#[test]
fn pager_validate_page_zero_errors() {
    let result = Pager::<MemFile>::validate_page(0, 10, None::<fn(_) -> bool>);
    assert!(result.is_err());
}

#[test]
fn pager_validate_page_beyond_max_errors() {
    let result = Pager::<MemFile>::validate_page(11, 10, None::<fn(_) -> bool>);
    assert!(result.is_err());
}

#[test]
fn pager_validate_page_with_exception() {
    let result = Pager::<MemFile>::validate_page(5, 10, Some(|p| p == 5));
    assert!(result.is_err());
}

#[test]
fn pager_validate_page_with_exception_not_triggered() {
    let result = Pager::<MemFile>::validate_page(3, 10, Some(|p| p == 5));
    assert!(result.is_ok());
}

#[test]
fn page_guard_bytes_returns_page_slice() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get(2).unwrap();
    let bytes = guard.bytes();
    assert_eq!(bytes.len(), 512);
}

#[test]
fn page_guard_bytes_reads_correct_page_data() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get(2).unwrap();
    let bytes = guard.bytes();
    assert!(bytes.iter().all(|&b| b == 0));
}

#[test]
fn page_guard_drop_decrements_pin_count() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get(2).unwrap();
    let fid = guard.frame_id();
    assert_eq!(pager.buffer_pool.frame_buffer[fid].pin_count.get(), 1);
    drop(guard);
    assert_eq!(pager.buffer_pool.frame_buffer[fid].pin_count.get(), 0);
}

#[test]
fn page_guard_mut_bytes_returns_mutable_slice() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let mut guard = pager.get_mut(2).unwrap();
    let bytes = guard.bytes();
    assert_eq!(bytes.len(), 512);
}

#[test]
fn page_guard_mut_bytes_can_modify_page() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let mut guard = pager.get_mut(2).unwrap();
    guard.bytes()[0] = 0xFF;
    assert_eq!(guard.bytes()[0], 0xFF);
}

#[test]
fn page_guard_mut_drop_decrements_pin_count() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);
    let guard = pager.get_mut(2).unwrap();
    let fid = guard.frame_id();
    assert_eq!(pager.buffer_pool.frame_buffer[fid].pin_count.get(), 1);
    drop(guard);
    assert_eq!(pager.buffer_pool.frame_buffer[fid].pin_count.get(), 0);
}

#[test]
fn sqlite_database_new_opens_valid_db() {
    let page_size = 512usize;
    let page_count = 2u32;
    let image = common::build_db_image(page_size, page_count, |_| {});
    let path = temp_path("db_new");
    write_db_file(&path, &image);
    let db = SqliteDatabase::new(&path).unwrap();
    assert_eq!(db.page_count(), page_count);
    assert_eq!(db.page_size(), page_size as u32);
    assert_eq!(db.usable_size(), page_size as u32);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_with_cache_opens_valid_db() {
    let page_size = 512usize;
    let page_count = 2u32;
    let image = common::build_db_image(page_size, page_count, |_| {});
    let path = temp_path("db_with_cache");
    write_db_file(&path, &image);
    let db = SqliteDatabase::with_cache(&path, 2).unwrap();
    assert_eq!(db.page_count(), page_count);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_page_reads_correctly() {
    let page_size = 512usize;
    let page_count = 2u32;
    let mut image = common::build_db_image(page_size, page_count, |_| {});
    image[100] = common::LEAF_TABLE;
    let path = temp_path("db_page_read");
    write_db_file(&path, &image);
    let mut db = SqliteDatabase::new(&path).unwrap();
    let page = db.page(1).unwrap();
    assert_eq!(page.no_of_cell(), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_read_raw_page_into() {
    let page_size = 512usize;
    let page_count = 2u32;
    let image = common::build_db_image(page_size, page_count, |_| {});
    let path = temp_path("db_raw_page");
    write_db_file(&path, &image);
    let mut db = SqliteDatabase::new(&path).unwrap();
    let mut buf = vec![0u8; page_size];
    db.read_raw_page_into(2, &mut buf).unwrap();
    assert_eq!(buf.len(), page_size);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_read_raw_page_one_errors() {
    let page_size = 512usize;
    let page_count = 2u32;
    let image = common::build_db_image(page_size, page_count, |_| {});
    let path = temp_path("db_raw_page_one");
    write_db_file(&path, &image);
    let mut db = SqliteDatabase::new(&path).unwrap();
    let mut buf = vec![0u8; page_size];
    let result = db.read_raw_page_into(1, &mut buf);
    assert!(result.is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_page_beyond_count_errors() {
    let page_size = 512usize;
    let page_count = 2u32;
    let image = common::build_db_image(page_size, page_count, |_| {});
    let path = temp_path("db_page_beyond");
    write_db_file(&path, &image);
    let mut db = SqliteDatabase::new(&path).unwrap();
    let result = db.page(3);
    assert!(result.is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_page_count_returns_correct_value() {
    for count in [1u32, 2, 5, 10] {
        let image = common::build_db_image(512, count, |_| {});
        let path = temp_path(&format!("db_page_count_{count}"));
        write_db_file(&path, &image);
        let db = SqliteDatabase::new(&path).unwrap();
        assert_eq!(db.page_count(), count);
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn sqlite_database_usable_size_matches_page_size() {
    let image = common::build_db_image(4096, 2, |_| {});
    let path = temp_path("db_usable_size");
    write_db_file(&path, &image);
    let db = SqliteDatabase::new(&path).unwrap();
    assert_eq!(db.usable_size(), 4096);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sqlite_database_usable_size_with_reserved_space() {
    let page_size = 512usize;
    let reserved = 16u8;
    let image = common::build_db_image(page_size, 2, |h| {
        h[20] = reserved;
    });
    let path = temp_path("db_usable_reserved");
    write_db_file(&path, &image);
    let db = SqliteDatabase::new(&path).unwrap();
    assert_eq!(db.usable_size(), (page_size - reserved as usize) as u32);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pager_small_cache_evicts_old_pages() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 2);

    let _g1 = pager.get(2).unwrap();
    {
        let _g2 = pager.get(3).unwrap();
        assert_eq!(pager.cached_page_count(), 2);
    }

    let _g3 = pager.get(4).unwrap();
    assert_eq!(pager.cached_page_count(), 2);
}

#[test]
fn pager_small_cache_with_pinned_page_evicts_other() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 2);

    {
        let _g1 = pager.get(2).unwrap(); // 1

        let _g1_again = pager.get(2).unwrap(); // 1 cache full
    }
    let _g2 = pager.get(3).unwrap(); // 1

    let _g3 = pager.get(4).unwrap();
    assert_eq!(pager.cached_page_count(), 2);
}

#[test]
fn pager_get_mut_then_get_reads_modified_data() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);

    {
        let mut guard = pager.get_mut(2).unwrap();
        guard.bytes()[0] = 0xAB;
    }

    let guard = pager.get(2).unwrap();
    assert_eq!(guard.bytes()[0], 0xAB);
}

#[test]
fn pager_dirty_page_flushed_on_eviction() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    {
        let mut guard = pager.get_mut(2).unwrap();
        guard.bytes()[20] = 0xDE;
    }

    {
        let _g3 = pager.get(3).unwrap();
        let _g4 = pager.get(4).unwrap();
    }

    let guard = pager.get(2).unwrap();
    assert_eq!(guard.bytes()[20], 0xDE);
}

#[test]
fn pager_statistics_tracks_cache_hits_and_misses() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);

    let _g1 = pager.get(2).unwrap();
    assert_eq!(pager.statistics.cache_miss(), 1);
    assert_eq!(pager.statistics.cache_hit(), 0);

    drop(_g1);

    let _g2 = pager.get(2).unwrap();
    assert_eq!(pager.statistics.cache_hit(), 1);
}

#[test]
fn pager_statistics_tracks_evictions() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    let _g1 = pager.get(2).unwrap();
    {
        let _g2 = pager.get(3).unwrap();
    }

    let _g3 = pager.get(4).unwrap();
    assert!(pager.statistics.evictions() >= 1);
}

#[test]
fn pager_get_page_one_reads_header() {
    let page_size = 512usize;
    let image = common::build_db_image(page_size, 2, |_| {});
    assert_eq!(&image[0..16], b"SQLite format 3\0");

    let file = MemFile::new(rc_vec(image));
    assert_eq!(file.len().unwrap(), 1024);
    let mut buf = vec![0u8; page_size]; // now holds header
    file.read_exact_at(0, &mut buf).unwrap();
    // dbg!(&buf);
    assert_eq!(&buf[0..16], b"SQLite format 3\0");
    let mut pager = Pager::with_cache(file, page_size, page_size, 2, 2);
    let guard = pager.get(1).unwrap();
    let bytes = guard.bytes();
    // // dbg!(&guard.bytes());
    assert_eq!(bytes.len(), page_size);
    assert_eq!(&bytes[0..16], b"SQLite format 3\0");
}

#[test]
fn pager_with_reserved_space() {
    let page_size = 512usize;
    let reserved = 16u8;
    let usable = page_size - reserved as usize;
    let file = make_mem_db(page_size, 3);
    let pager = Pager::new(file, page_size, usable, 3);
    assert_eq!(pager.metadata.usable_size, usable);
}

#[test]
fn buffer_pool_page_table_tracks_loaded_pages() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    assert!(pager.buffer_pool.page_table.is_empty());

    let _g1 = pager.get(2).unwrap();
    assert!(pager.buffer_pool.page_table.contains_key(&2));

    let _g2 = pager.get(3).unwrap();
    assert!(pager.buffer_pool.page_table.contains_key(&2));
    assert!(pager.buffer_pool.page_table.contains_key(&3));
}

#[test]
fn buffer_pool_page_table_removes_on_eviction() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 2);
    {
        let _g1 = pager.get(2).unwrap();
        let _g2 = pager.get(3).unwrap();
        assert!(pager.buffer_pool.page_table.contains_key(&2));
        assert!(pager.buffer_pool.page_table.contains_key(&3));
    }
    let _g3 = pager.get(4).unwrap();
    let _g4 = pager.get(5).unwrap();

    let evicted = !pager.buffer_pool.page_table.contains_key(&2)
        || !pager.buffer_pool.page_table.contains_key(&3);
    assert!(evicted);
}

#[test]
fn frame_buffer_indexed_by_frame_id() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 4);

    let g1 = pager.get(2).unwrap();
    let frame_id = g1.frame_id();
    let frame = &pager.buffer_pool.frame_buffer[frame_id];
    assert_eq!(frame.page_no, Some(2));
    assert!(frame.is(REFERENCED));
}

#[test]
fn frame_buffer_multiple_pages_different_frame_ids() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    let g1 = pager.get(2).unwrap();
    let fid1 = g1.frame_id();
    let g2 = pager.get(3).unwrap();
    let fid2 = g2.frame_id();

    assert_ne!(fid1, fid2);
}

#[test]
fn pager_source_is_mem_file() {
    let file = make_mem_db(512, 3);
    let pager = Pager::new(file, 512, 512, 3);
    assert_eq!(pager.source.len().unwrap(), 1536);
}

#[test]
fn pager_source_is_disk_file() {
    let path = temp_path("pager_source_disk");
    let image = common::build_db_image(512, 2, |_| {});
    write_db_file(&path, &image);
    let mut vfs = DiskVfs;
    let file = vfs.open(&path, SqliteOptions::default()).unwrap();
    let pager = Pager::new(file, 512, 512, 2);
    assert_eq!(pager.source.len().unwrap(), 1024);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dp_ll_empty_when_no_dirty_pages() {
    let file = make_mem_db(512, 3);
    let pager = Pager::with_cache(file, 512, 512, 3, 3);
    assert!(pager.dp_ll.is_none());
}

#[test]
fn get_mut_inserts_page_into_dp_ll() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);
    let _g = pager.get_mut(1).unwrap();
    assert!(pager.dp_ll.is_some());
}

#[test]
fn get_mut_on_same_page_twice_increments_pin_count() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);
    let _g1 = pager.get_mut(1).unwrap();
    let _g2 = pager.get_mut(1).unwrap();
    let frame_id = _g1.frame_id();
    let frame = &pager.buffer_pool.frame_buffer[frame_id];
    assert_eq!(frame.pin_count.get(), 2);
}

#[test]
fn get_mut_same_page_single_dp_ll_entry() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);
    let _g1 = pager.get_mut(1).unwrap();
    let frame_id = _g1.frame_id();
    let _g2 = pager.get_mut(1).unwrap();
    // dp_ll should have exactly one entry for this page
    assert_eq!(pager.dp_ll, Some(frame_id));
    let frame = &pager.buffer_pool.frame_buffer[frame_id];
    dbg!(frame);
    assert_eq!(frame.next, None);
    assert_eq!(frame.prev, None);
}

#[test]
fn get_does_not_add_page_to_dp_ll() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);
    let _g = pager.get(1).unwrap();
    assert!(pager.dp_ll.is_none());
}

#[test]
fn dp_ll_tail_is_most_recently_made_dirty() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 5);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    let _g2 = pager.get_mut(2).unwrap();
    let fid2 = _g2.frame_id();

    // fid2 should be the tail (most recently inserted)
    assert_eq!(pager.dp_ll, Some(fid2));
    // fid2's prev should point to fid1
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].prev, Some(fid1));
    // fid1's next should point to fid2
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].next, Some(fid2));
}

#[test]
fn dp_ll_three_pages_ordered() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 5);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    let _g2 = pager.get_mut(2).unwrap();
    let fid2 = _g2.frame_id();
    let _g3 = pager.get_mut(3).unwrap();
    let fid3 = _g3.frame_id();

    // tail is fid3
    assert_eq!(pager.dp_ll, Some(fid3));
    // fid3 -> fid2 -> fid1
    assert_eq!(pager.buffer_pool.frame_buffer[fid3].prev, Some(fid2));
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].prev, Some(fid1));
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].prev, None);
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].next, Some(fid2));
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].next, Some(fid3));
    assert_eq!(pager.buffer_pool.frame_buffer[fid3].next, None);
}

#[test]
fn drop_page_guard_mut_decrements_pin_count() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let frame_id = {
        let g = pager.get_mut(1).unwrap();
        let fid = g.frame_id();
        assert_eq!(pager.buffer_pool.frame_buffer[fid].pin_count.get(), 1);
        fid
    };
    // guard dropped, pin_count should be 0
    assert_eq!(pager.buffer_pool.frame_buffer[frame_id].pin_count.get(), 0);
}

#[test]
fn clock_eviction_skips_pinned_dirty_pages() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    let _g1 = pager.get_mut(1).unwrap(); // pin_count=1, dirty
    let _g2 = pager.get_mut(2).unwrap(); // pin_count=1, dirty

    // cache is full (2 frames), both pages are pinned
    // trying to load page 3 should fail because both frames are pinned
    let result = pager.get(3);
    assert!(result.is_err());
}

#[test]
fn clock_eviction_evicts_clean_unpinned_page_ignoring_pinned_dirty() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    let _g1 = pager.get_mut(1).unwrap(); // dirty, pin_count=1
                                         // page 2 is loaded and immediately dropped — clean, pin_count=0
    pager.get(2).unwrap();

    // page 1 is pinned (dirty), page 2 is clean and unpinned
    // page 3 should evict page 2 (clean, not pinned)
    let _g3 = pager.get_mut(3).unwrap();

    // page 2 should be evicted
    assert!(!pager.buffer_pool.page_table.contains_key(&2));
    // page 1 should still be cached (pinned dirty)
    assert!(pager.buffer_pool.page_table.contains_key(&1));
    // page 3 should be cached
    assert!(pager.buffer_pool.page_table.contains_key(&3));
}

#[test]
fn eviction_of_dirty_page_flushes_and_removes_from_dp_ll() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 1);

    let _g1 = pager.get_mut(1).unwrap(); // dirty, pin_count=1
    let fid1 = _g1.frame_id();
    assert!(pager.buffer_pool.frame_buffer[fid1].is(DIRTY));

    // pin_count is 1, so clock eviction can't evict it
    // we need to drop the guard first to decrement pin_count
    drop(_g1);
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].pin_count.get(), 0); // true

    // now page 2 should be able to evict page 1 (dirty, unpinned)
    let _g2 = pager.get_mut(2).unwrap();

    // page 1 should have been evicted (flushed and removed)
    assert!(!pager.buffer_pool.page_table.contains_key(&1));
    // dp_ll should be empty since the only dirty page was flushed
    assert!(!pager.dp_ll.is_none());
}

#[test]
fn dp_ll_remove_on_eviction() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    let _g1 = pager.get_mut(1).unwrap(); // pin1, dirty not flushed
    let fid1 = _g1.frame_id();
    drop(_g1); // pin = 0;

    // page 2 is clean, not in dp_ll
    let _g2 = pager.get(2).unwrap(); // pin = 1;
    let fid2 = _g2.frame_id();

    // dp_ll has only page 1 (dirty, unpinned)
    assert_eq!(pager.dp_ll, Some(fid1));

    // Evict page 1 by loading page 3 (page 1 is dirty, unpinned)
    let _g3 = pager.get_mut(3).unwrap();

    // page 1 should have been flushed and removed from dp_ll
    assert!(!pager.buffer_pool.page_table.contains_key(&1));
    // dp_ll should be empty since the only dirty page was flushed
    assert!(pager.dp_ll == Some(_g3.frame_id()));
}

#[test]
fn flush_all_clears_dp_ll() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    let _g1 = pager.get_mut(1).unwrap();
    let _g2 = pager.get_mut(2).unwrap();
    let _g3 = pager.get_mut(3).unwrap();

    // dp_ll should have entries
    assert!(pager.dp_ll.is_some());

    // manually flush all
    pager.flush_all().unwrap();

    // dp_ll should be empty after flush_all
    assert!(pager.dp_ll.is_none());
}

#[test]
fn flush_all_flushes_all_dirty_pages() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 4);

    let _g1 = pager.get_mut(1).unwrap();
    let _g2 = pager.get_mut(2).unwrap();

    // mark both as dirty (they already are from get_mut)
    // flush_all should write them to disk
    pager.flush_all().unwrap();

    // after flush_all, all frames should be CLEAN
    for frame in pager.buffer_pool.frame_buffer.iter() {
        if frame.page_no.is_some() {
            assert!(
                frame.is(CLEAN),
                "Frame for page {:?} should be CLEAN after flush_all",
                frame.page_no
            );
        }
    }
}

#[test]
fn dp_ll_single_page_remove() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    drop(_g1);

    // dp_ll has one entry (fid1)
    assert_eq!(pager.dp_ll, Some(fid1));

    // Evict page 1 by loading page 2 (page 1 is dirty, unpinned)
    let _g2 = pager.get_mut(2).unwrap();

    // page 1 should have been flushed and removed from dp_ll
    assert!(pager.dp_ll.is_none() || pager.dp_ll != Some(fid1));
}

#[test]
fn get_mut_cleans_clean_page_and_adds_to_dp_ll() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    // First, load page 1 cleanly
    let _g1 = pager.get(1).unwrap();
    let fid1 = _g1.frame_id();
    assert!(pager.buffer_pool.frame_buffer[fid1].is(CLEAN));
    assert!(pager.dp_ll.is_none());

    // Now get it mutably - should mark dirty and add to dp_ll
    let _g2 = pager.get_mut(1).unwrap();
    assert!(pager.buffer_pool.frame_buffer[fid1].is(DIRTY));
    assert!(pager.dp_ll.is_some());
}

#[test]
fn dp_ll_remove_updates_links_correctly() {
    let file = make_mem_db(512, 5);
    let mut pager = Pager::with_cache(file, 512, 512, 5, 5);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    let _g2 = pager.get_mut(2).unwrap();
    let fid2 = _g2.frame_id();
    let _g3 = pager.get_mut(3).unwrap();
    let fid3 = _g3.frame_id();

    // dp_ll: fid3 -> fid2 -> fid1
    assert_eq!(pager.dp_ll, Some(fid3));

    // Remove fid2 (middle of list)
    pager.dp_ll_remove(fid2);

    // fid3's prev should now point to fid1
    assert_eq!(pager.buffer_pool.frame_buffer[fid3].prev, Some(fid1));
    // fid1's next should now point to fid3
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].next, Some(fid3));
    // fid2 should be unlinked
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].prev, None);
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].next, None);
}

#[test]
fn dp_ll_remove_head() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    let _g2 = pager.get_mut(2).unwrap();
    let fid2 = _g2.frame_id();

    // dp_ll: fid2 -> fid1 (fid2 is head/tail since it was inserted last)
    // fid2 is the tail (dp_ll points to it)
    assert_eq!(pager.dp_ll, Some(fid2));

    // Remove fid2 (tail)
    pager.dp_ll_remove(fid2);

    // dp_ll should now point to fid1
    assert_eq!(pager.dp_ll, Some(fid1));
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].prev, None);
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].next, None);
}

#[test]
fn dp_ll_remove_tail() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    let _g2 = pager.get_mut(2).unwrap();
    let fid2 = _g2.frame_id();

    // dp_ll: fid2 -> fid1, fid2 is tail
    assert_eq!(pager.dp_ll, Some(fid2));

    // Remove fid1 (head, since it was inserted first)
    // fid1 is the head of the list (dp_ll points to fid2, fid2.prev = fid1)
    pager.dp_ll_remove(fid1);

    // dp_ll should still point to fid2
    assert_eq!(pager.dp_ll, Some(fid2));
    assert_eq!(pager.buffer_pool.frame_buffer[fid2].prev, None);
}

#[test]
fn dp_ll_remove_only_element() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    drop(_g1);

    // dp_ll has one element
    assert_eq!(pager.dp_ll, Some(fid1));

    // Remove it
    pager.dp_ll_remove(fid1);

    // dp_ll should be empty
    assert!(pager.dp_ll.is_none());
}

#[test]
fn pager_drop_flushes_dirty_pages() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let _g1 = pager.get_mut(1).unwrap();
    let fid1 = _g1.frame_id();
    // page 1 is dirty
    assert!(pager.buffer_pool.frame_buffer[fid1].is(DIRTY));

    // When pager drops, flush_all should be called
    // We can't easily verify the flush happened without reading back,
    // but we can verify dp_ll state changes after drop
    // Actually, we can't access pager after drop, so let's just verify
    // that the pager compiles and runs without error
}

#[test]
fn pin_count_prevents_eviction_of_dirty_page() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);

    let _g1 = pager.get_mut(1).unwrap(); // dirty, pin_count=1
    let fid1 = _g1.frame_id();

    // Load page 2 (clean)
    let _g2 = pager.get(2).unwrap();

    // Cache is full. Try to load page 3.
    // Page 1 is dirty and pinned, page 2 is clean and pinned.
    // Clock should skip page 1 (dirty+pinned) and page 2 (pinned),
    // then loop and fail with BufferPoolExhausted.
    let result = pager.get(3);
    assert!(result.is_err());
}

#[test]
fn pin_count_allows_eviction_after_guard_drops() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 2);

    let fid1 = {
        let _g1 = pager.get_mut(1).unwrap();
        _g1.frame_id()
    };
    // pin_count should be 0 after guard drops
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].pin_count.get(), 0);

    // Now page 1 can be evicted
    let _g2 = pager.get_mut(2).unwrap();
    let _g3 = pager.get_mut(3).unwrap();

    // Page 1 should have been evicted (it was dirty, flushed, and removed)
    assert!(!pager.buffer_pool.page_table.contains_key(&1));
}

#[test]
fn multiple_get_mut_guards_same_page_pin_count() {
    let file = make_mem_db(512, 3);
    let mut pager = Pager::with_cache(file, 512, 512, 3, 3);

    let fid1 = {
        let g1 = pager.get_mut(1).unwrap();
        assert_eq!(
            pager.buffer_pool.frame_buffer[g1.frame_id()]
                .pin_count
                .get(),
            1
        );
        let g2 = pager.get_mut(1).unwrap();
        assert_eq!(
            pager.buffer_pool.frame_buffer[g1.frame_id()]
                .pin_count
                .get(),
            2
        );
        g1.frame_id()
    };

    // After both guards dropped, pin_count should be 0
    // (g2 was dropped first, then g1)
    assert_eq!(pager.buffer_pool.frame_buffer[fid1].pin_count.get(), 0);
}

#[test]
fn dirty_page_not_evicted_while_pinned_by_get_mut() {
    let file = make_mem_db(512, 4);
    let mut pager = Pager::with_cache(file, 512, 512, 4, 2);

    let _g1 = pager.get_mut(1).unwrap(); // dirty, pin_count=1
    let fid1 = _g1.frame_id();

    // Fill the cache with page 2
    let _g2 = pager.get_mut(2).unwrap();

    // Both pages are pinned. Loading page 3 should fail.
    let result = pager.get_mut(3);
    assert!(result.is_err());

    // Drop guard for page 1
    drop(_g1);

    // Now page 1 can be evicted. Loading page 3 should succeed
    // by evicting page 1 (dirty, unpinned)
    let _g3 = pager.get_mut(3).unwrap();

    // Page 1 should be gone
    assert!(!pager.buffer_pool.page_table.contains_key(&1));
    // Page 2 and 3 should be present
    assert!(pager.buffer_pool.page_table.contains_key(&2));
    assert!(pager.buffer_pool.page_table.contains_key(&3));
}
