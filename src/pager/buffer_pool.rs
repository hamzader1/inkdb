use super::frame::{CLEAN, DIRTY, REFERENCED};
use super::frame::{Frame, FrameId, FrameIndex};
use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::pager::PageNo;
use crate::util::sqlite_assert_one;
use std::collections::HashMap;
use std::ptr::NonNull;

const CACHE_SIZE: usize = 4096;

#[rustfmt::skip]
pub struct BufferPool {
    page_table:              HashMap<PageNo, FrameId>,
    page_buffer:             Box<[u8]>,
    frame_buffer:            Box<[Frame]>, // Frame Id used to index
    free_frames:             Vec<FrameId>,  // Frame Id to Index frame_buffer
    clock_hand:              FrameIndex,
    page_size:               usize,
    dirty_pages_linked_list: Option<FrameId>,
}
impl BufferPool {
    pub fn new(page_size: usize) -> Self {
        Self::with_cache(CACHE_SIZE, page_size)
    }

    pub fn with_cache(cache_size: usize, page_size: usize) -> Self {
        let cache_cap: usize = cache_size
            .checked_mul(page_size)
            .expect("Overflow while trying to multiply");
        let page_buffer = Self::owned_buffer::<u8>(cache_cap);

        let frame_buffer = Self::owned_buffer::<Frame>(cache_size);

        let free_frames: Vec<FrameId> = (0..cache_size).collect();

        Self {
            page_table: HashMap::new(),
            page_buffer,
            frame_buffer,
            free_frames,
            dirty_pages_linked_list: None,
            page_size,
            clock_hand: 0,
        }
    }
    fn evict_page(&mut self, page_no: PageNo, frame_id: FrameId) -> Result<(), SqliteError> {
        sqlite_assert_one(
            self.page_table.contains_key(&page_no)
                && *self.page_table.get(&page_no).unwrap() == frame_id,
            SqliteError::Internal(format!(
                "buffer pool evict: frame {frame_id} does not map page {page_no}"
            )),
        )?;
        self.page_table.remove(&page_no);
        self.frame_buffer[frame_id] = Frame::default();
        Ok(())
    }

    fn owned_buffer<T: Clone + Default>(size: usize) -> Box<[T]> {
        vec![T::default(); size].into_boxed_slice()
    }
    pub fn as_ptr_mut(&mut self) -> NonNull<Self> {
        unsafe { NonNull::new_unchecked(self as *mut BufferPool) }
    }

    pub fn lookup(&self, page: PageNo) -> Option<FrameId> {
        self.page_table.get(&page).copied()
    }
    pub fn cached_count(&self) -> usize {
        self.frame_buffer.len() - self.free_frames.len()
    }
    pub fn frame_bytes(&self, id: FrameId) -> &[u8] {
        let offset = id * self.page_size;
        &self.page_buffer[offset..offset + self.page_size]
    }
    pub fn frame_bytes_mut(&mut self, id: FrameId) -> &mut [u8] {
        let offset = id * self.page_size;
        &mut self.page_buffer[offset..offset + self.page_size]
    }
    fn dp_ll_insert(&mut self, frame_id: FrameId) {
        let frame = &mut self.frame_buffer[frame_id];
        frame.prev = self.dirty_pages_linked_list;
        frame.next = None;
        if let Some(db_ll_tail) = self.dirty_pages_linked_list {
            let ll_tail_frame = &mut self.frame_buffer[db_ll_tail];
            ll_tail_frame.next = Some(frame_id);
        }
        self.dirty_pages_linked_list = Some(frame_id);
    }
    fn dp_ll_remove(&mut self, frame_id: FrameId) {
        // safe to unwrap since we want to remove a Node,
        // so logically we at lease have one node
        let is_tail = frame_id == self.dirty_pages_linked_list.unwrap(); // if this panics, we have a bug

        let frame = &mut self.frame_buffer[frame_id];
        let next = frame.next;
        let prev = frame.prev;
        // in case this returned the buffer pool,
        // should not handle its old pointers so it breaks the list
        frame.prev = None;
        frame.next = None;

        if is_tail && prev.is_none() {
            self.dirty_pages_linked_list = None;
            return;
        }
        if let Some(next_frame_id) = next {
            let next_frame = &mut self.frame_buffer[next_frame_id];
            next_frame.prev = prev
        }
        if let Some(prev_frame_id) = prev {
            let prev_frame = &mut self.frame_buffer[prev_frame_id];
            prev_frame.next = next;
            if is_tail {
                self.dirty_pages_linked_list = Some(prev_frame_id);
            }
        }
    }

    pub fn acquire(&mut self, page_no: PageNo) -> SqliteResult<Acquire> {
        if let Some(frameid) = self.lookup(page_no) {
            let frame = &self.frame_buffer[frameid];
            frame.set(REFERENCED);
            frame.incr_pin_count();
            return Ok(Acquire::Hit(frameid));
        }

        // Case [A] we do have free frames
        if let Some(frameid) = self.free_frames.pop() {
            self.page_table.insert(page_no, frameid);
            self.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN | REFERENCED, 1);
            return Ok(Acquire::Miss {
                frameid,
                evicted: None,
            });
        }

        let mut clock_hand = self.clock_hand;
        let start = clock_hand;
        let mut laps = 0;
        let buffer_len = self.frame_buffer.len();
        let frameid: usize = loop {
            if clock_hand == start {
                laps += 1;
                // TODO: Explain why more than 2 laps
                if laps > 2 {
                    return Err(SqliteError::BufferPoolExhausted);
                }
            }
            let frame = &mut self.frame_buffer[clock_hand];
            if frame.pin_count.get() == 0 {
                if frame.is(REFERENCED) {
                    frame.clear(REFERENCED);
                } else {
                    break clock_hand;
                }
            }
            clock_hand = (clock_hand + 1) % buffer_len;
        };
        self.clock_hand = clock_hand;
        let frame = &self.frame_buffer[frameid];
        let frame_page_no = frame.page_no.unwrap();
        let mut was_dirty = false;
        if frame.is(DIRTY) {
            was_dirty = true;
            self.dp_ll_remove(frameid);
        }
        self.evict_page(frame_page_no, frameid)?;
        self.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN | REFERENCED, 1);
        Ok(Acquire::Miss {
            frameid,
            evicted: Some(Evicted::new(frame_page_no, was_dirty)),
        })
    }

    pub fn mark_dirty(&mut self, frame_id: FrameId) {
        let frame = &mut self.frame_buffer[frame_id];
        frame.set(REFERENCED);
        if frame.is(DIRTY) {
            return;
        }
        frame.clear(CLEAN);
        frame.set(DIRTY);
        self.dp_ll_insert(frame_id);
    }
    pub fn borrow_state(&self, frame_id: FrameId) -> i16 {
        self.frame_buffer[frame_id].borrow.get()
    }
    pub fn mark_clean(&mut self, frame_id: FrameId) {
        if self.frame_buffer[frame_id].is(DIRTY) {
            self.dp_ll_remove(frame_id);
        }
        self.frame_buffer[frame_id].reset_to(CLEAN);
    }
    pub fn pin(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].incr_pin_count();
    }
    pub fn unpin(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].decr_pin_count();
    }
    pub fn borrow(&self, frameid: FrameId, page_no: PageNo) -> SqliteResult<()> {
        let frame = &self.frame_buffer[frameid];
        if frame.borrow.get() < 0 {
            return Err(SqliteError::Runtime(format!(
                "Page no '{}' already borrowed as mut",
                page_no
            )));
        }
        frame.borrow.set(frame.borrow.get() + 1);
        Ok(())
    }
    pub fn exclusive_borrow(&self, frameid: FrameId, page_no: PageNo) -> SqliteResult<()> {
        let frame = &self.frame_buffer[frameid];
        if frame.borrow.get() != 0 {
            return Err(SqliteError::Runtime(format!(
                "Page no '{}' already borrowed as ref",
                page_no
            )));
        }
        frame.borrow.set(-1);
        Ok(())
    }

    pub fn release_frame(&self, frame_id: FrameId) {
        let frame = &self.frame_buffer[frame_id];
        let current = frame.borrow.get();
        assert!(
            current >= 0,
            "current counter is less than 0, page already borrowed as mut"
        );
        frame.borrow.set(frame.borrow.get() - 1);
    }
    pub fn reset_frame(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].borrow.set(0);
    }

    pub fn pop_dirty(&mut self) -> Option<(PageNo, FrameId)> {
        let curr = self.dirty_pages_linked_list?;
        let page_no = self.frame_buffer[curr].page_no.unwrap();
        self.dirty_pages_linked_list = self.frame_buffer[curr].prev;
        if let Some(new_tail) = self.dirty_pages_linked_list {
            self.frame_buffer[new_tail].next = None;
        }
        self.frame_buffer[curr].next = None;
        self.frame_buffer[curr].prev = None;
        Some((page_no, curr))
    }

    pub fn with_frame_as_ref<F, R>(&self, frameid: FrameId, f: F) -> R
    where
        F: for<'a> FnOnce(&'a Frame) -> R,
    {
        f(&self.frame_buffer[frameid])
    }
}

#[derive(Debug)]
pub enum Acquire {
    Hit(FrameId), // temporary solution for dp_ll
    Miss {
        frameid: FrameId,
        evicted: Option<Evicted>,
    },
}

#[derive(Debug)]
pub struct Evicted {
    pub page_no: PageNo,
    pub was_dirty: bool,
}

impl Evicted {
    pub fn new(page_no: PageNo, was_dirty: bool) -> Self {
        Self { page_no, was_dirty }
    }
}
