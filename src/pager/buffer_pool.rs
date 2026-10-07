use super::frame::{CLEAN, DIRTY, REFERENCED};
use super::frame::{Frame, FrameId, FrameIndex};
use crate::InkResult;
use crate::errors::InkError;
use crate::pager::pager::PageNo;
use crate::util::assert_one;
use std::collections::HashMap;
use std::ptr::NonNull;

/// The page cache: room for a fixed number of pages, plus the index that says
/// which page is in which slot.
///
/// A slot is called a frame. The page table maps a page number to the frame
/// holding it, the page buffer holds every frame's bytes, and the
/// frame buffer holds the frame records. When a page has to come in and no frame
/// is free, the clock hand walks the frames and hands back the first one that is
/// neither pinned nor recently used.
#[rustfmt::skip]
#[derive(Debug)]
pub struct BufferPool {
    page_table:              HashMap<PageNo, FrameId>,
    page_buffer:             Box<[u8]>,
    frame_buffer:            Box<[Frame]>,
    free_frames:             Vec<FrameId>,  // FrameId to Index frame_buffer
    clock_hand:              FrameIndex,
    page_size:               usize,
    dirty_pages_linked_list: Option<FrameId>,
}
impl BufferPool {
    /// Make a pool with room for the default number of pages.
    pub fn new(page_size: usize, cache_size: usize) -> Self {
        Self::with_cache(cache_size, page_size)
    }

    /// Make a pool with room for this many pages of this size.
    ///
    /// # Panics
    /// When the cache size and the page size multiplied together overflow. The
    /// bytes for every frame are set aside up front, so the product has to fit
    /// in a `usize`.
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
    /// Drop a page from the pool. The frame itself is cleared by the caller.
    ///
    /// # Errors
    /// When the page table does not map this page to this frame, which would
    /// mean the pool is already in an inconsistent state.
    fn evict_page(&mut self, page_no: PageNo, frame_id: FrameId) -> Result<(), InkError> {
        assert_one(
            self.page_table
                .get(&page_no)
                .is_some_and(|tableframe_id| *tableframe_id == frame_id),
            InkError::InternalFmt(format!(
                "buffer pool evict: frame {frame_id} does not map page {page_no}"
            )),
        )?;
        // We remove the page from the page table
        self.page_table.remove(&page_no);
        // Reset the frame
        self.frame_buffer[frame_id] = Frame::default();
        Ok(())
    }

    /// A buffer of this many default values, boxed so its address is stable.
    fn owned_buffer<T: Clone + Default>(size: usize) -> Box<[T]> {
        vec![T::default(); size].into_boxed_slice()
    }

    /// A raw pointer to the pool, so a guard can pin and unpin frames without
    /// holding a borrow that would stop the caller from using the pool.
    pub fn as_ptr_mut(&mut self) -> NonNull<Self> {
        unsafe { NonNull::new_unchecked(self as *mut BufferPool) }
    }

    /// The frame holding this page, or nothing when the page is not cached.
    pub fn lookup(&self, page: PageNo) -> Option<FrameId> {
        self.page_table.get(&page).copied()
    }

    /// How many frames are holding a page.
    pub fn cached_count(&self) -> usize {
        self.frame_buffer.len() - self.free_frames.len()
    }

    /// The bytes of a frame, for reading.
    pub fn frame_bytes(&self, id: FrameId) -> &[u8] {
        let offset = id * self.page_size;
        &self.page_buffer[offset..offset + self.page_size]
    }

    /// The bytes of a frame, for writing.
    pub fn frame_bytes_mut(&mut self, id: FrameId) -> &mut [u8] {
        let offset = id * self.page_size;
        &mut self.page_buffer[offset..offset + self.page_size]
    }

    /// Add a frame to the dirty list, which is kept roughly in the order pages
    /// were first marked dirty.
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
    /// Take a frame out of the dirty list.
    ///
    /// # Panics
    /// When the frame is not in the list. The callers only remove frames they
    /// have already put in, so a panic here means the list lost track of a
    /// frame somewhere else.
    fn dp_ll_remove(&mut self, frame_id: FrameId) {
        let is_tail = frame_id == self.dirty_pages_linked_list.unwrap();

        let frame = &mut self.frame_buffer[frame_id];
        let next = frame.next;
        let prev = frame.prev;

        // Erase them first
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

    /// Find a frame for a page, reading it from disk if it is not already in
    /// the cache.
    ///
    /// A cached page is a hit and its pin count is increased. Otherwise, a free
    /// frame is used. If no free frame is available, the clock hand picks a frame
    /// to evict. If that frame is dirty, its contents must be handled before the
    /// frame can be reused for the new page.
    /// # Example
    ///
    /// ```text
    /// Let's walk through an example.
    ///
    /// We have four frames: F1 through F4.
    /// R=0 means the frame has not been referenced.
    /// R=1 means the frame has been referenced.
    ///
    /// F1: Referenced
    /// F2: Referenced + Pinned
    /// F3: Referenced
    /// F4: Referenced + Pinned
    ///
    ///                 +------------+
    ///                 |    F1:     |
    ///                 |    R=1     |
    ///                 +------------+
    ///                       ^
    /// +------------+        |          +------------+
    /// |    F4:     |        |          |    F2:     |
    /// | R=1+Pinned |                   | R=1+Pinned |
    /// +------------+                   +------------+
    ///
    ///                 +------------+
    ///                 |    F3:     |
    ///                 |    R=1     |
    ///                 +------------+
    ///
    /// On the first run, the clock is pointing at F1. It is referenced but not
    /// pinned, so we clear its reference bit and move to the next frame.
    ///
    ///                 +------------+
    ///                 |    F1:     |
    ///                 |    R=0     |
    ///                 +------------+
    ///
    /// +------------+                   +------------+
    /// |    F4:     |        ------>    |    F2:     |
    /// | R=1+Pinned |                   | R=1+Pinned |
    /// +------------+                   +------------+
    ///
    ///                 +------------+
    ///                 |    F3:     |
    ///                 |    R=1     |
    ///                 +------------+
    ///
    /// F2 is referenced and pinned, so we skip it and move the clock forward.
    /// Since F2 is still in use, we cannot evict it.
    ///
    /// After the first run, the clock ends up here:
    ///
    ///                 +------------+
    ///                 |    F1:     |
    ///                 |    R=0     |
    ///                 +------------+
    ///                       ^
    /// +------------+        |          +------------+
    /// |    F4:     |        |          |    F2:     |
    /// | R=1+Pinned |                   | R=1+Pinned |
    /// +------------+                   +------------+
    ///                 +------------+
    ///                 |    F3:     |
    ///                 |    R=0     |
    ///                 +------------+
    ///
    /// At the beginning of the second run, the clock is already pointing at F1.
    /// F1 is not pinned and its reference bit is cleared, so it is safe to evict.
    ///
    /// If the second run did not evict any frame, it means all frames are pinned,
    /// as explained below.
    ///
    /// ```
    ///
    /// # Errors
    /// [`InkError::BufferPoolExhausted`] when the clock hand goes round twice
    /// without finding a frame it may take, which means every frame is pinned.
    pub fn acquire(&mut self, page_no: PageNo) -> InkResult<Acquire> {
        if let Some(frameid) = self.lookup(page_no) {
            let frame = &self.frame_buffer[frameid];
            frame.set(REFERENCED);
            frame.incr_pin_count();
            return Ok(Acquire::Hit(frameid));
        }

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
                // Two laps is enough for the hand to clear every reference flag
                // it set on the way round, so a third lap means nothing can be
                // taken and the pool is full of pinned pages.
                if laps > 2 {
                    return Err(InkError::BufferPoolExhausted);
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
        // Checking whether the frame is dirty is not just about removing it from
        // the dirty page linked list (dp_ll). A dirty frame also needs to be flushed
        // to disk before it can be evicted, otherwise its changes would be lost.
        // However, flushing it directly to disk would bypass the journal. If the
        // journal does not know about this page reaching disk, a later rollback could
        // restore every other page to its state before the transaction while leaving
        // this page with its newer data, breaking the consistency guarantee.
        if frame.is(DIRTY) {
            was_dirty = true;
            self.dp_ll_remove(frameid);
        }
        self.evict_page(frame_page_no, frameid)?;
        self.page_table.insert(page_no, frameid);
        self.frame_buffer[frameid] = Frame::new(Some(page_no), CLEAN | REFERENCED, 1);
        Ok(Acquire::Miss {
            frameid,
            evicted: Some(Evicted::new(frame_page_no, was_dirty)),
        })
    }

    /// Mark a frame as changed, putting it in the dirty list so it gets written
    /// back. A frame that is already dirty is left alone.
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
    /// How the frame is borrowed: positive for readers, negative for a writer,
    /// zero when it is not borrowed at all.
    pub fn borrow_state(&self, frame_id: FrameId) -> i16 {
        self.frame_buffer[frame_id].borrow.get()
    }

    /// Mark a frame as matching the file again, taking it out of the dirty list
    /// if it was in there.
    pub fn mark_clean(&mut self, frame_id: FrameId) {
        if self.is_linked(frame_id) {
            self.dp_ll_remove(frame_id);
        }
        self.frame_buffer[frame_id].reset_to(CLEAN);
    }
    /// Whether a frame is in the dirty list.
    fn is_linked(&self, frame_id: FrameId) -> bool {
        self.dirty_pages_linked_list == Some(frame_id)
            || self.frame_buffer[frame_id].next.is_some()
            || self.frame_buffer[frame_id].prev.is_some()
    }
    /// Take a pin on a frame, which keeps it from being evicted.
    pub fn pin(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].incr_pin_count();
    }
    /// Release a pin on a frame.
    pub fn unpin(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].decr_pin_count();
    }
    /// Note that a frame is being borrowed for reading.
    ///
    /// # Errors
    /// When the frame is currently borrowed for writing, since a reader and a
    /// writer cannot hold the same page at once.
    // This feature is currently disabled
    pub fn borrow(&self, frameid: FrameId, page_no: PageNo) -> InkResult<()> {
        let frame = &self.frame_buffer[frameid];
        if frame.borrow.get() < 0 {
            return Err(InkError::runtime(format!(
                "Page no '{}' already borrowed as mut",
                page_no
            )));
        }
        frame.borrow.set(frame.borrow.get() + 1);
        Ok(())
    }
    /// Note that a frame is being borrowed for writing.
    ///
    /// # Errors
    /// When the frame is borrowed at all, since a writer needs the page to
    /// itself.
    // This feature is currently disabled
    pub fn exclusive_borrow(&self, frameid: FrameId, page_no: PageNo) -> InkResult<()> {
        let frame = &self.frame_buffer[frameid];
        if frame.borrow.get() != 0 {
            return Err(InkError::runtime(format!(
                "Page no '{}' already borrowed as ref",
                page_no
            )));
        }
        frame.borrow.set(-1);
        Ok(())
    }

    /// Give back one reader's hold on a frame.
    ///
    /// # Panics
    /// When the frame is not held by any reader, which includes the case where
    /// it is held by a writer.
    // This feature is currently disabled
    pub fn release_frame(&self, frame_id: FrameId) {
        let frame = &self.frame_buffer[frame_id];
        let current = frame.borrow.get();
        assert!(
            current >= 0,
            "current counter is less than 0, page already borrowed as mut"
        );
        frame.borrow.set(frame.borrow.get() - 1);
    }
    /// Drop a writer's hold on a frame.
    pub fn reset_frame(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].borrow.set(0);
    }

    /// Take the oldest frame off the dirty list, with the page it holds.
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

    /// Run a function against a frame, which is the only way to look at one
    /// from outside the pool without taking a borrow of the pool itself.
    pub fn with_frame_as_ref<F, R>(&self, frameid: FrameId, f: F) -> R
    where
        F: for<'a> FnOnce(&'a Frame) -> R,
    {
        f(&self.frame_buffer[frameid])
    }

    /// Put a frame's bytes back to what they were, which is how a rollback
    /// undoes a change that never reached disk.
    ///
    /// # Panics
    /// When the bytes are not exactly one page long.
    pub fn restore_bytes(&mut self, id: FrameId, bytes: &[u8]) {
        assert!(bytes.len() == self.page_size);
        let start = id * self.page_size;
        let end = start + self.page_size;
        self.page_buffer[start..end].copy_from_slice(bytes);
    }
}

/// What came of asking the pool for a page.
#[derive(Debug)]
pub enum Acquire {
    /// The page was already in the cache, in this frame.
    Hit(FrameId),
    /// The page was not cached and this frame was found for it. When a frame
    /// had to be taken from another page, that page is reported in `evicted`.
    Miss {
        frameid: FrameId,
        evicted: Option<Evicted>,
    },
}

/// A page that was handed back to make room for another one.
#[derive(Debug)]
pub struct Evicted {
    /// The page that lost its frame.
    pub page_no: PageNo,
    /// Whether the page has changes that have not yet been written to disk,
    /// meaning it must be flushed before the frame can be reused.
    pub was_dirty: bool,
}

impl Evicted {
    pub fn new(page_no: PageNo, was_dirty: bool) -> Self {
        Self { page_no, was_dirty }
    }
}
