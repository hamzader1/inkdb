use inkdb::pager::buffer_pool::{Acquire, BufferPool};

const PS: usize = 64;

fn pool(cache: usize) -> BufferPool {
    BufferPool::with_cache(cache, PS)
}

fn unpin_all(p: &BufferPool, pages: &[u32]) {
    for page in pages {
        let fid = p.lookup(*page).unwrap();
        p.unpin(fid);
    }
}

#[test]
fn acquire_miss_then_hit() {
    let mut p = pool(4);
    let first = p.acquire(1).unwrap();
    assert!(matches!(first, Acquire::Miss { evicted: None, .. }));
    let fid = match first {
        Acquire::Miss { frameid, .. } => frameid,
        _ => unreachable!(),
    };
    assert_eq!(p.lookup(1), Some(fid));
    assert!(matches!(p.acquire(1).unwrap(), Acquire::Hit(_)));
    unpin_all(&p, &[1, 1]);
}

#[test]
fn frame_bytes_roundtrip() {
    let mut p = pool(2);
    let fid = match p.acquire(1).unwrap() {
        Acquire::Miss { frameid, .. } => frameid,
        _ => unreachable!(),
    };
    p.frame_bytes_mut(fid).fill(0xAB);
    assert!(p.frame_bytes(fid).iter().all(|b| *b == 0xAB));
    p.unpin(fid);
}

#[test]
fn mark_dirty_pop_dirty_lifo_and_idempotent() {
    let mut p = pool(4);
    for page in [1, 2] {
        let fid = match p.acquire(page).unwrap() {
            Acquire::Miss { frameid, .. } => frameid,
            _ => unreachable!(),
        };
        p.unpin(fid);
    }
    let f1 = p.lookup(1).unwrap();
    let f2 = p.lookup(2).unwrap();
    p.mark_dirty(f1);
    p.mark_dirty(f2);
    p.mark_dirty(f1);
    assert_eq!(p.pop_dirty(), Some((2, f2)));
    assert_eq!(p.pop_dirty(), Some((1, f1)));
    assert_eq!(p.pop_dirty(), None);
}

#[test]
fn clock_evicts_cold_victim() {
    let mut p = pool(2);
    for page in [1, 2] {
        let fid = match p.acquire(page).unwrap() {
            Acquire::Miss { frameid, .. } => frameid,
            _ => unreachable!(),
        };
        p.unpin(fid);
    }
    let m = p.acquire(3).unwrap();
    match m {
        Acquire::Miss {
            evicted: Some(ev), ..
        } => {
            assert!(!ev.was_dirty);
            assert!(ev.page_no == 1 || ev.page_no == 2);
        }
        _ => panic!("expected eviction"),
    };
    let fid = p.lookup(3).unwrap();
    p.unpin(fid);
}

#[test]
fn dirty_eviction_reports_was_dirty() {
    let mut p = pool(2);
    for page in [1, 2] {
        let fid = match p.acquire(page).unwrap() {
            Acquire::Miss { frameid, .. } => frameid,
            _ => unreachable!(),
        };
        p.unpin(fid);
    }
    p.mark_dirty(p.lookup(1).unwrap());
    p.mark_dirty(p.lookup(2).unwrap());
    let m = p.acquire(3).unwrap();
    match m {
        Acquire::Miss {
            evicted: Some(ev), ..
        } => {
            assert!(ev.was_dirty);
            assert!(ev.page_no == 1 || ev.page_no == 2);
        }
        _ => panic!("expected dirty eviction"),
    };
    let fid = p.lookup(3).unwrap();
    p.unpin(fid);
}

#[test]
fn pinned_frames_exhaust_pool() {
    let mut p = pool(2);
    let _ = p.acquire(1).unwrap();
    let _ = p.acquire(2).unwrap();
    assert!(p.acquire(3).is_err());
    let f1 = p.lookup(1).unwrap();
    p.unpin(f1);
    let m = p.acquire(3).unwrap();
    assert!(matches!(m, Acquire::Miss { .. }));
    let f2 = p.lookup(2).unwrap();
    let f3 = p.lookup(3).unwrap();
    p.unpin(f2);
    p.unpin(f3);
}
