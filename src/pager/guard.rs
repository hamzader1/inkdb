#![feature(unboxed_closures)]
#![feature(fn_traits)]
use crate::pager::{buffer_pool, frame::FrameId};

use super::buffer_pool::BufferPool;
use super::frame::Frame;
use std::{marker::PhantomData, ptr::NonNull};

#[derive(Debug, PartialEq)]
pub enum BorrowState {
    Ref,
    RefMut,
}

#[derive(Debug)]
pub struct PageGuard {
    buffer_pool: NonNull<BufferPool>,
    frame_id: FrameId,
    bytes: NonNull<[u8]>,
    state: BorrowState,
    _marker: PhantomData<BufferPool>,
}
impl PageGuard {
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
    pub fn bytes_as_ref(&self) -> &[u8] {
        unsafe { self.bytes.as_ref() }
    }
    pub fn bytes_as_mut(&mut self) -> Option<&mut [u8]> {
        if self.state == BorrowState::RefMut {
            return unsafe { Some(self.bytes.as_mut()) };
        }
        None
    }

    pub fn bytes_as_mut_unchecked(&mut self) -> &mut [u8] {
        self.bytes_as_mut().unwrap()
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
