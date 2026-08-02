use crate::pager::{buffer_pool, frame::FrameId};

use super::buffer_pool::BufferPool;
use std::{marker::PhantomData, ptr::NonNull};

#[derive(Debug)]
pub enum PageBytes<'b> {
    RefBytes(&'b [u8]),
    RefMutBytes(&'b mut [u8]),
}

#[derive(Debug)]
pub struct PageGuard<'b> {
    bufferpool: NonNull<BufferPool>,
    frame_id: FrameId,
    page_bytes: PageBytes<'b>,
    _marker: PhantomData<BufferPool>,
}
impl<'b> PageBytes<'b> {
    pub fn new_ref(bytes: &'b [u8]) -> Self {
        Self::RefBytes(bytes)
    }

    pub fn new_refmut(bytes: &'b mut [u8]) -> Self {
        Self::RefMutBytes(bytes)
    }
    fn bytes_as_ref(&self) -> &'_ [u8] {
        match self {
            Self::RefBytes(bytes) => bytes,

            // allowed to get &[u8] if you have &mut [u8]
            Self::RefMutBytes(bytes) => bytes,
        }
    }
    fn bytes_as_mut(&mut self) -> &mut [u8] {
        if let Self::RefMutBytes(bytes) = self {
            bytes
        } else {
            panic!("You can't get mut bytes from immutable page shared reference")
        }
    }
}
impl<'b> PageGuard<'b> {
    pub fn new_ref(bufferpool: NonNull<BufferPool>, bytes: &'b [u8], frame_id: FrameId) -> Self {
        Self {
            bufferpool,
            frame_id,
            page_bytes: PageBytes::new_ref(bytes),
            _marker: PhantomData,
        }
    }

    pub fn new_refmut(
        bufferpool: NonNull<BufferPool>,
        bytes: &'b mut [u8],
        frame_id: FrameId,
    ) -> Self {
        Self {
            bufferpool,
            frame_id,
            page_bytes: PageBytes::new_refmut(bytes),
            _marker: PhantomData,
        }
    }
    pub fn bytes_as_ref(&self) -> &[u8] {
        self.page_bytes.bytes_as_ref()
    }

    pub fn bytes_as_mut(&mut self) -> &mut [u8] {
        self.page_bytes.bytes_as_mut()
    }
}

impl<'b> Drop for PageGuard<'b> {
    fn drop(&mut self) {
        unsafe {
            self.bufferpool.as_mut().release(self.frame_id);
        }
    }
}

impl<'b> From<&'b [u8]> for PageBytes<'b> {
    fn from(value: &'b [u8]) -> Self {
        Self::RefBytes(value)
    }
}

impl<'b> From<&'b mut [u8]> for PageBytes<'b> {
    fn from(value: &'b mut [u8]) -> Self {
        Self::RefMutBytes(value)
    }
}
