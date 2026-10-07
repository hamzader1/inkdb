use crate::pager::frame::FrameId;

use super::buffer_pool::BufferPool;
use std::{marker::PhantomData, ptr::NonNull};

/// How a page is currently borrowed from the cache.
#[derive(Debug, PartialEq)]
pub enum BorrowState {
    Ref,
    RefMut,
}

/// A handle on a page in the cache.
///
/// While a guard is alive the page stays pinned, so the pool will not evict it
/// or hand its frame to another page. Dropping the guard releases the pin. The
/// guard points at the pool and at the page's bytes, and
/// the pin is what keeps both valid for as long as the guard lives.
#[derive(Debug)]
pub struct PageGuard {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    bytes: NonNull<[u8]>,
    state: BorrowState,
    _marker: PhantomData<BufferPool>,
}
impl PageGuard {
    /// Build a guard over a frame and its bytes.
    pub fn new(
        buffer_pool: NonNull<BufferPool>,
        frame_id: FrameId,
        bytes: NonNull<[u8]>,
        state: BorrowState,
    ) -> Self {
        Self {
            buffer_pool,
            frame_id,
            bytes,
            state,
            _marker: PhantomData,
        }
    }

    /// The page's bytes, for reading.
    pub fn bytes(&self) -> &[u8] {
        unsafe { self.bytes.as_ref() }
    }

    /// The page's bytes, for writing, or nothing when the guard was handed out
    /// for reading only.
    pub fn bytes_as_mut(&mut self) -> Option<&mut [u8]> {
        if self.state == BorrowState::RefMut {
            return unsafe { Some(self.bytes.as_mut()) };
        }
        None
    }

    /// The page's bytes, for writing, without checking that the guard allows it.
    ///
    /// # Panics
    /// When the guard was handed out for reading only, which is the same check
    /// [`PageGuard::bytes_as_mut`] makes.
    pub fn bytes_as_mut_unchecked(&mut self) -> &mut [u8] {
        self.bytes_as_mut().unwrap()
    }

    /// Which frame the page is held in.
    pub fn frame_id(&self) -> FrameId {
        self.frame_id
    }
}

/// Release the frame's pin when the guard goes out of scope.
impl Drop for PageGuard {
    fn drop(&mut self) {
        unsafe {
            self.buffer_pool.as_mut().unpin(self.frame_id);
        }
    }
}
