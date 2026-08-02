use super::frame::FRAME_SIZE;
use super::frame::{Frame, FrameId, FrameIndex};
use crate::format::page::PageNo;
use crate::pager::page;
use std::collections::HashMap;

const CACHE_CAPACITY: usize = 4096;
#[rustfmt::skip]
pub struct BufferPool {
    page_map    : HashMap<PageNo, FrameId>,
    page_buffer : Box<[u8]>,
    frame_buffer: Box<[Frame]>,
    free_frames : Vec<FrameId>,
    clock_hand  : FrameIndex
}

impl BufferPool {
    fn new(page_size: usize) -> Self {
        // todo: check overflow of CacheCap * Psize
        let page_buffer = Self::make_box_buffer::<u8>(CACHE_CAPACITY * page_size);
        // todo: check overflow of CacheCap * Fsize
        let frame_buffer = Self::make_box_buffer::<Frame>(CACHE_CAPACITY * FRAME_SIZE);
        let free_frames = Vec::<FrameId>::with_capacity(CACHE_CAPACITY);

        Self {
            page_map: HashMap::new(),
            page_buffer,
            frame_buffer,
            free_frames,
            clock_hand: 0,
        }
    }
    pub fn make_box_buffer<T: Clone + Default>(size: usize) -> Box<[T]> {
        vec![T::default(); size].into_boxed_slice()
    }
}
