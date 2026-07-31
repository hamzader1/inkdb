use std::io::{Cursor, Read};
use std::ptr::slice_from_raw_parts;

const U32_SIZE: usize = size_of::<u32>();
use crate::bytes::read_u32_be;
use crate::util::{sqlite_assert_one, sqlite_assert_with_corrupt_err};
use crate::vfs::file::SqliteFile;
use crate::{to_int, DbError, SqliteDatabase};

use super::page::PageNumber;
use crate::SqliteDatabaseError;

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
pub struct OverflowPageRef<'a> {
    pub next: PageNumber,
    pub data: &'a [u8],
}
impl<'a> OverflowPageRef<'a> {
    pub fn new<T: AsRef<[u8]> + ?Sized>(bytes: &'a T, usable_size: usize) -> Result<Self, DbError> {
        let data = bytes.as_ref();
        sqlite_assert_with_corrupt_err(
            data.len() >= usable_size,
            "not enough bytes in overflow page",
        )?;

        let next_page_buffer = match data[0..4].as_array::<U32_SIZE>() {
            Some(buf) => buf,
            _ => {
                return Err(DbError::Corrupt(
                    "Failed to parse next overflow page from overflow page".into(),
                ))
            }
        };
        let next_page = u32::from_be_bytes(*next_page_buffer);
        let data = &data[4..(usable_size)];

        Ok(Self {
            next: next_page,
            data,
        })
    }
}

impl<S: SqliteFile> SqliteDatabase<S> {
    pub fn read_overflow_payload(
        &mut self,
        mut local_payload_bytes: Vec<u8>,
        total_payload_length: usize,
        first_overflow_page: PageNumber,
    ) -> Result<Vec<u8>, SqliteDatabaseError> {
        let mut remaining = total_payload_length
            .checked_sub(local_payload_bytes.len())
            .ok_or(SqliteDatabaseError::CorruptedPage {
                page: first_overflow_page, // needs to be change later to the parent page
                reason: "local payload exceeds total payload length".into(),
            })?;
        let mut buffer = vec![0u8; self.header.database_page_size as usize];
        let mut current_page = first_overflow_page;
        while remaining > 0 {
            self.read_raw_page_into(current_page, &mut buffer)?;
            let overflow_page = OverflowPageRef::new(&buffer, self.usable_size() as _)?;
            let bytes_to_read: usize = remaining.min(overflow_page.data.len());
            local_payload_bytes.extend_from_slice(&overflow_page.data[..bytes_to_read]);
            remaining -= bytes_to_read;
            if remaining == 0 {
                if overflow_page.next != 0 {
                    return Err(SqliteDatabaseError::CorruptedPage {
                        page: current_page,
                        reason: "overflow chain continues after payload is complete".into(),
                    });
                }
                break;
            }
            if overflow_page.next == 0 {
                return Err(SqliteDatabaseError::CorruptedPage {
                    page: current_page,
                    reason: "overflow chain ends before payload is complete".into(),
                });
            }
            current_page = overflow_page.next;
        }

        sqlite_assert_one(
            local_payload_bytes.len() == total_payload_length,
            DbError::Corrupt("assembled payload length mismatch".into()),
        )?;

        Ok(local_payload_bytes)
    }
}
