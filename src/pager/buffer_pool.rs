use super::frame::FRAME_SIZE;
use super::frame::{Frame, FrameId, FrameIndex};
use crate::errors::SqliteError;
use crate::format::page::PageNo;
use crate::pager::{frame, page};
use crate::util::sqlite_assert_one;
use crate::DbError;
use std::collections::HashMap;
use std::ptr::NonNull;

const CACHE_CAPACITY: usize = 4096;
#[rustfmt::skip]
// TODO: after getting things done, change pub to pub(super)
pub struct BufferPool {
    pub page_table    : HashMap<PageNo, FrameId>,
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
            page_table: HashMap::new(),
            page_buffer,
            frame_buffer,
            free_frames,
            clock_hand: 0,
        }
    }
    pub fn evict_page(&mut self, page_no: PageNo, frame_id: FrameId) -> Result<(), SqliteError> {
        sqlite_assert_one(
            *self.page_table.get(&page_no).unwrap() == frame_id,
            SqliteError::Corrupt("Frame ID mistmatch while trying to evict the page".into()),
        )?;
        self.page_table.remove(&page_no);
        self.frame_buffer[frame_id] = Frame::default();
        Ok(())
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
