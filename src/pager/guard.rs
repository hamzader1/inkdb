use crate::pager::{buffer_pool, frame::FrameId};

use super::buffer_pool::BufferPool;
use std::{marker::PhantomData, ptr::NonNull};

enum PageBytes<'b> {
    RefBytes(&'b Vec<u8>),
    RefMutBytes(&'b mut Vec<u8>),
}
struct PageGuard<'b> {
    bufferpool: NonNull<BufferPool>,
    frame_id: FrameId,
    bytes: PageBytes<'b>,
    _marker: PhantomData<BufferPool>,
}
impl<'b> PageBytes<'b> {
    pub fn new_ref(bytes: &'b Vec<u8>) -> Self {
        Self::RefBytes(bytes)
    }

    pub fn new_refmut(bytes: &'b mut Vec<u8>) -> Self {
        Self::RefMutBytes(bytes)
    }
    fn get_bytes(&self) -> &'_ Vec<u8> {
        match self {
            Self::RefBytes(bytes) => bytes,

            // allowed to get &Vec if you have &mut Vec
            Self::RefMutBytes(bytes) => bytes,
        }
    }
    fn get_bytes_mut(&mut self) -> &mut Vec<u8> {
        if let Self::RefMutBytes(bytes) = self {
            bytes
        } else {
            panic!("You can't get mut bytes from immutable page shared reference")
        }
    }
}
impl<'b> PageGuard<'b> {
    pub fn new_ref(bufferpool: NonNull<BufferPool>, bytes: &'b Vec<u8>, frame_id: FrameId) -> Self {
        Self {
            bufferpool,
            frame_id,
            bytes: PageBytes::new_ref(bytes),
            _marker: PhantomData,
        }
    }

    pub fn new_refmut(
        bufferpool: NonNull<BufferPool>,
        bytes: &'b mut Vec<u8>,
        frame_id: FrameId,
    ) -> Self {
        Self {
            bufferpool,
            frame_id,
            bytes: PageBytes::new_refmut(bytes),
            _marker: PhantomData,
        }
    }
    pub fn bytes_as_ref(&self) -> &Vec<u8> {
        self.page_bytes.get_bytes()
    }

    pub fn bytes_as_mut(&mut self) -> &mut Vec<u8> {
        self.page_bytes.get_bytes_mut()
    }
}

impl<'b> Drop for PageGuard<'b> {
    fn drop(&mut self) {
        unsafe {
            self.bufferpool.as_mut().release(self.frame_id);
        }
    }
}
