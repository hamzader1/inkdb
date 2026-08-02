use super::frame::FRAME_SIZE;
use super::frame::{Frame, FrameId, FrameIndex};
use crate::format::page::PageNo;
use crate::pager::page;
use std::collections::HashMap;
use std::ptr::NonNull;

const CACHE_CAPACITY: usize = 4096;
#[rustfmt::skip]
// TODO: after getting things done, change pub to pub(super)
pub struct BufferPool {
    pub page_map    : HashMap<PageNo, FrameId>,
    pub page_buffer : Box<[u8]>,
    pub frame_buffer: Box<[Frame]>, // Frame Id used to index
    pub free_frames : Vec<FrameId>, // Frame Id to Index frame_buffer
    pub clock_hand  : FrameIndex
}

impl BufferPool {
    pub fn new(page_size: usize) -> Self {
        // todo: check overflow of CacheCap * Psize
        let page_buffer = Self::make_owned_buffer::<u8>(CACHE_CAPACITY * page_size);
        // todo: check overflow of CacheCap * Fsize
        let frame_buffer = Self::make_owned_buffer::<Frame>(CACHE_CAPACITY * FRAME_SIZE);

        let free_frames: Vec<FrameId> = (0..CACHE_CAPACITY).collect();

        Self {
            page_map: HashMap::new(),
            page_buffer,
            frame_buffer,
            free_frames,
            clock_hand: 0,
        }
    }
    pub fn release(&self, frame_id: FrameId) {
        self.frame_buffer[frame_id].decr_pin_count();
    }
    pub fn make_owned_buffer<T: Clone + Default>(size: usize) -> Box<[T]> {
        vec![T::default(); size].into_boxed_slice()
    }
    pub fn as_ptr_mut(&mut self) -> NonNull<Self> {
        unsafe { NonNull::new_unchecked(self as *mut BufferPool) }
    }
}
