use std::io::{Cursor, Read};
use std::ptr::slice_from_raw_parts;

const U32_SIZE: usize = size_of::<u32>();
use crate::bytes::read_u32_be;
use crate::{to_int, DbError};

use super::page::PageNumber;

pub struct OverflowPage<'a> {
    pub next: PageNumber,
    pub data: &'a [u8],
}
impl<'a> OverflowPage<'a> {
    pub fn new<T: AsRef<[u8]> + ?Sized>(bytes: &'a T, usable_size: usize) -> Result<Self, DbError> {
        let data = bytes.as_ref();
        let next_page_buffer = data[0..4].as_array::<U32_SIZE>().unwrap();
        let next_page = u32::from_be_bytes(*next_page_buffer);
        let data = &data[4..(usable_size)];

        Ok(Self {
            next: next_page,
            data,
        })
    }
}

/*
   ** X is U-35 for table btree leaf pages or ((U-12)*64/255)-23 for index pages.
   ** M is always ((U-12)*32/255)-23.
   ** Let K be M+((P-M)%(U-4)).
   ** If P<=X then all P bytes of payload are stored directly
       on the btree page without overflow.

   ** If P>X and K<=X then the first K bytes of P are stored
       on the btree page and the remaining P-K bytes are stored
       on overflow pages.

   ** If P>X and K>X then the first M bytes of P are stored on
       the btree page and the remaining P-M bytes are stored on
       overflow pages.
*/
pub const fn compute_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = u - 35;
    if p <= x {
        return p;
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + (p - m) % (u - 4);
        if k <= x {
            return k;
        } else {
            return m;
        }
    }
}
