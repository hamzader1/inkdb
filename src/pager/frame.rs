use crate::format::page::PageNo;
use crate::size_of;

const FREE: u8 = 1 << 0;
const CLEAN: u8 = 1 << 1;
const DIRTY: u8 = 1 << 2;
const REFERENCED: u8 = 1 << 3;

pub const FRAME_SIZE: usize = size_of!(Frame);
pub type FrameId = usize;
pub type FrameIndex = usize;
#[derive(Clone, Debug)]
pub struct Frame {
    page_no: Option<PageNo>,
    flags: u8,
    pin_count: u8,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            page_no: None,
            flags: FREE,
            pin_count: 0,
        }
    }
}
