use crate::pager::{buffer_pool, frame::FrameId};

use super::buffer_pool::BufferPool;
use super::frame::Frame;
use std::{marker::PhantomData, ptr::NonNull};

#[derive(Debug)]
pub struct PageGuard {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    bytes: NonNull<[u8]>,
    _marker: PhantomData<BufferPool>,
}
#[derive(Debug)]
pub struct PageGuardMut {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    bytes: NonNull<[u8]>,
    _marker: PhantomData<BufferPool>,
}

impl PageGuard {
    pub fn new(buffer_pool: NonNull<BufferPool>, frame_id: FrameId, bytes: NonNull<[u8]>) -> Self {
        Self {
            buffer_pool,
            frame_id,
            bytes,
            _marker: PhantomData,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        unsafe { self.bytes.as_ref() }
    }
    pub fn frame_id(&self) -> FrameId {
        self.frame_id
    }
}

impl PageGuardMut {
    pub fn new(buffer_pool: NonNull<BufferPool>, frame_id: FrameId, bytes: NonNull<[u8]>) -> Self {
        Self {
            buffer_pool,
            frame_id,
            bytes,
            _marker: PhantomData,
        }
    }
    pub fn bytes(&mut self) -> &mut [u8] {
        unsafe { self.bytes.as_mut() }
    }
    pub fn frame_id(&self) -> FrameId {
        self.frame_id
    }
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        unsafe {
            self.buffer_pool.as_mut().free_page(self.frame_id);
        }
    }
}

impl Drop for PageGuardMut {
    fn drop(&mut self) {
        unsafe {
            self.buffer_pool.as_mut().free_page(self.frame_id);
        }
    }
}
