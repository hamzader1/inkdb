use crate::pager::pager::PageNo;
use crate::size_of;
use std::cell::Cell;

pub const FREE: u8 = 1 << 0;
pub const CLEAN: u8 = 1 << 1;
pub const DIRTY: u8 = 1 << 2;
pub const REFERENCED: u8 = 1 << 3;

pub const FRAME_SIZE: usize = size_of!(Frame);
pub type FrameId = usize;
pub type FrameIndex = usize;
#[derive(Clone, Debug)]
pub struct Frame {
    pub page_no: Option<PageNo>,
    pub flags: Cell<u8>,
    pub pin_count: Cell<u8>,
    pub next: Option<FrameId>,
    pub prev: Option<FrameId>,
}
impl Frame {
    pub fn new(page_no: Option<PageNo>, flags: u8, pin_count: u8) -> Self {
        Self {
            page_no,
            flags: Cell::new(flags),
            pin_count: Cell::new(pin_count),
            next: None,
            prev: None,
        }
    }
    pub fn is(&self, flag: u8) -> bool {
        assert!(flag == FREE || flag == CLEAN || flag == DIRTY || flag == REFERENCED);
        self.flags.get() & flag != 0
    }

    pub fn set(&self, flag: u8) {
        self.flags.set(self.flags.get() | flag);
    }

    pub fn clear(&self, flag: u8) {
        self.flags.set(self.flags.get() & !flag);
    }

    pub fn reset_to(&self, flag: u8) {
        self.flags.swap(&Cell::new(flag));
    }
    pub fn incr_pin_count(&self) {
        let curr_cnt = self.pin_count.get();
        assert!((curr_cnt as u16 + 1) < u8::MAX as _, "pin count overflow");
        self.pin_count.set(curr_cnt + 1);
    }

    pub fn decr_pin_count(&self) {
        let curr_cnt = self.pin_count.get();
        self.pin_count.set(curr_cnt - 1);
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            page_no: None,
            flags: Cell::new(FREE),
            pin_count: Cell::new(0),
            next: None,
            prev: None,
        }
    }
}
