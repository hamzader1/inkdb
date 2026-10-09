use crate::pager::pager::PageNo;
use std::cell::Cell;

/// The flag for a slot that holds no page.
pub const FREE: u8 = 1 << 0;
/// The flag for a frame whose bytes match what is on disk.
pub const CLEAN: u8 = 1 << 1;
/// The flag for a frame whose bytes have been changed and no longer match the
/// file.
pub const DIRTY: u8 = 1 << 2;
/// Marks a frame as recently referenced.
pub const REFERENCED: u8 = 1 << 3;

/// How many bytes one frame takes in memory.
pub const FRAME_SIZE: usize = size_of::<Frame>();
/// Where a frame sits in the pool's frame buffer.
pub type FrameId = usize; /* Replace this with a `FrameId(usize)` new type */
/// A place in the frame buffer, which is where the clock hand points.
pub type FrameIndex = usize;
/// One slot in the buffer pool: the page it holds and everything the pool keeps
/// track of for that page.
///
/// The flags and both counters sit in cells, so they can be read and changed
/// through a shared reference. A guard holds only a borrow of the pool while a
/// page is in use, and it still has to be able to mark the frame as referenced
/// or take a pin, which a plain field would not allow.
#[derive(Clone, Debug)]
pub struct Frame {
    pub page_no: Option<PageNo>,
    pub flags: Cell<u8>,
    pub pin_count: Cell<u32>,
    pub next: Option<FrameId>,
    pub prev: Option<FrameId>,
    pub borrow: Cell<i16>,
}
impl Frame {
    pub fn new(page_no: Option<PageNo>, flags: u8, pin_count: u32) -> Self {
        Self {
            page_no,
            flags: Cell::new(flags),
            pin_count: Cell::new(pin_count),
            next: None,
            prev: None,
            borrow: Cell::new(0),
        }
    }
    /// Whether a flag is set.
    ///
    /// # Panics
    /// When the flag is not one of the four above.
    pub fn is(&self, flag: u8) -> bool {
        assert!(flag == FREE || flag == CLEAN || flag == DIRTY || flag == REFERENCED);
        self.flags.get() & flag != 0
    }

    /// Turn a flag on, leaving the others as they are.
    pub fn set(&self, flag: u8) {
        self.flags.set(self.flags.get() | flag);
    }

    /// Turn a flag off, leaving the others as they are.
    pub fn clear(&self, flag: u8) {
        self.flags.set(self.flags.get() & !flag);
    }

    /// Throw the flags away and keep only this one.
    pub fn reset_to(&self, flag: u8) {
        self.flags.swap(&Cell::new(flag));
    }
    /// Take one more pin on the frame, which keeps the pool from evicting it.
    pub fn incr_pin_count(&self) {
        let curr_cnt = self.pin_count.get();
        self.pin_count.set(curr_cnt + 1);
    }

    /// Release one pin, so the frame can be evicted once none are left.
    pub fn decr_pin_count(&self) {
        let curr_cnt = self.pin_count.get();
        self.pin_count.set(curr_cnt - 1);
    }
}

/// A free frame holding no page, with no pins and no flags but `FREE`.
impl Default for Frame {
    fn default() -> Self {
        Self {
            page_no: None,
            flags: Cell::new(FREE),
            pin_count: Cell::new(0),
            next: None,
            prev: None,
            borrow: Cell::new(0),
        }
    }
}
