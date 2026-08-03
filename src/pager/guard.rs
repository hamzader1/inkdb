use crate::pager::{buffer_pool, frame::FrameId};

use super::buffer_pool::BufferPool;
use super::frame::Frame;
use std::{marker::PhantomData, ptr::NonNull};

// #[derive(Debug)]
// pub enum PageBytes<'b> {
//     RefBytes(&'b [u8]),
//     RefMutBytes(&'b mut [u8]),
// }

#[derive(Debug)]
pub struct PageGuard<'b> {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    page_bytes: &'b [u8],
    _marker: PhantomData<BufferPool>,
}
#[derive(Debug)]
pub struct PageGuardMut<'b> {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    page_bytes: &'b mut [u8],
    _marker: PhantomData<BufferPool>,
}

impl<'b> PageGuard<'b> {
    pub fn new(buffer_pool: NonNull<BufferPool>, frame_id: FrameId, bytes: &'b [u8]) -> Self {
        Self {
            buffer_pool,
            frame_id,
            page_bytes: bytes,
            _marker: PhantomData,
        }
    }
    pub fn bytes(&self) -> &'b [u8] {
        self.page_bytes
    }
}

impl<'b> PageGuardMut<'b> {
    pub fn new(buffer_pool: NonNull<BufferPool>, frame_id: FrameId, bytes: &'b mut [u8]) -> Self {
        Self {
            buffer_pool,
            frame_id,
            page_bytes: bytes,
            _marker: PhantomData,
        }
    }
    pub fn bytes(&self) -> &'b mut [u8] {
        self.page_bytes
    }
}

impl<'b> Drop for PageGuard<'b> {
    fn drop(&mut self) {
        unsafe {
            self.buffer_pool.as_mut().release(self.frame_id);
        }
    }
}

impl<'b> Drop for PageGuardMut<'b> {
    fn drop(&mut self) {
        unsafe {
            self.buffer_pool.as_mut().release(self.frame_id);
        }
    }
}
