use std::io::{Cursor, Read};

use crate::bytes::read_u32_be;
use crate::{to_int, DbError};

use super::page::PageNumber;

pub struct OverflowPage {
    pub next: PageNumber,
    pub data: Vec<u8>,
}
impl OverflowPage {
    pub fn new(bytes: Vec<u8>, usable_size: usize) -> Result<Self, DbError> {
        let mut cursor = Cursor::new(&bytes);
        // let next_page_buffer = &bytes[0..4];
        let next = read_u32_be(&mut cursor)?;
        let mut data = vec![0u8; usable_size - 4];
        cursor.read_exact(&mut data)?;
        Ok(Self { next, data })
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
