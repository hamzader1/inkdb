use crate::format::page::PageNo;

const FREE: u8 = 1 << 0;
const CLEAN: u8 = 1 << 1;
const DIRTY: u8 = 1 << 2;
const REFERENCED: u8 = 1 << 3;
struct Frame {
    page_no: PageNo,
    flags: u8,
    pin_count: u8,
}
