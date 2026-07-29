#![allow(unused, dead_code)]

mod bytes;
mod errors;
mod format;
mod macros;
mod util;
use errors::SqliteDatabaseError;
use format::header::SqliteDatabaseHeader;
use format::overflow::compute_local_payload_size;
use format::page::BTreePage;
pub use format::varint::{decode_varint, encode_varint};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::task::ready;
pub type DbError = SqliteDatabaseError;

use self::format::cell::BTreeCellType;
use self::format::freelist::FreeList;
use self::format::overflow::OverflowPageRef;
use self::format::page::{CellPointer, PageNumber};

pub struct SqliteDatabse {
    file: File,
    header: SqliteDatabaseHeader,
}
impl SqliteDatabse {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, SqliteDatabaseError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(db_path)
            .map_err(|err| SqliteDatabaseError::DatabaseOpenFailure(err))?;
        let header = SqliteDatabaseHeader::parse(&mut file)?;
        Ok(Self { file, header })
    }
    pub fn header(&self) -> &'_ SqliteDatabaseHeader {
        &self.header
    }

    pub fn page(&mut self, page_no: PageNumber) -> Result<BTreePage, SqliteDatabaseError> {
        self.validate_page(page_no, None::<fn() -> bool>)?;
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        self.file.seek(SeekFrom::Start(offset as u64))?;

        let mut buff = vec![0u8; page_size as usize];
        self.file.read_exact(&mut buff)?;

        BTreePage::parse(
            buff,
            page_no,
            page_size as usize,
            (page_size - self.header.reserved_space as u32) as usize,
        )
    }
    pub fn usable_size(&self) -> u32 {
        self.header.database_page_size - self.header.reserved_space as u32
    }

    pub fn read_raw_page_into(
        &mut self,
        page_no: PageNumber,
        buff: &mut Vec<u8>,
    ) -> Result<(), SqliteDatabaseError> {
        if page_no == 1 {
            return Err(SqliteDatabaseError::Corrupt(
                "Page no '1' cant be used as raw page".into(),
            ));
        }
        self.validate_page(page_no, None::<fn() -> bool>);
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        self.file.seek(SeekFrom::Start(offset as u64))?;

        self.file.read_exact(buff)?;

        Ok(())
    }

    fn validate_page<F>(
        &mut self,
        page_no: PageNumber,
        exception: Option<F>,
    ) -> Result<(), SqliteDatabaseError>
    where
        F: Fn() -> bool,
    {
        if let Some(exc) = exception {
            if exc() {
                return Err(SqliteDatabaseError::Corrupt("Exception Failed".into()));
            }
        }
        // TODO
        // let clos = |page: u32, other: u32| {
        //     if page == other {
        //         return Ok(());
        //     } else {
        //         return Err(SqliteDatabaseError::CellOverlap);
        //     }
        // };
        if page_no == 0 {
            return Err(SqliteDatabaseError::Corrupt(
                "page number cannot be zero".into(),
            ));
        } else if page_no > self.header.database_size_in_pages {
            return Err(SqliteDatabaseError::Corrupt(
                "page number is outside the database".into(),
            ));
        }

        Ok(())
    }

    fn read_overflow_payload(
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
        // let mut next_page = overflow_page.next;
        while remaining > 0 {
            let overflow_page_buffer = self.read_raw_page_into(current_page, &mut buffer)?;
            let mut overflow_page = OverflowPageRef::new(&buffer, self.usable_size() as _)?;
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

        assert!(
            local_payload_bytes.len() == total_payload_length,
            "Assembleing bytes wen't wrong" // todo
        );
        Ok(local_payload_bytes)
    }

    pub fn cell_payload(
        &mut self,
        page: &BTreePage,
        cell_idx: u16,
    ) -> Result<Vec<u8>, SqliteDatabaseError> {
        let cell = page.cell(cell_idx)?;
        assert!(
            cell.cell_type() != BTreeCellType::TableInterior,
            "TableInterior has no cells"
        );
        let local_payload = cell.payload();
        let mut payload = Vec::<u8>::new();
        // let mut cursor = Cursor::new(page.bytes());
        // cursor.seek(SeekFrom::Start( as u64))?;

        payload.extend_from_slice(&page.bytes()[local_payload.start..local_payload.end]);
        if cell.overflow_page().is_none() {
            assert!(
                payload.len() == cell.cell_payload_len() as usize,
                " payload buffer length does not match original cell payload len"
            );
            return Ok(payload);
        }
        // has overflow
        self.read_overflow_payload(
            payload,
            cell.cell_payload_len() as usize,
            cell.overflow_page().unwrap(),
        )
    }

    pub fn page_count(&self) -> u32 {
        self.header.database_size_in_pages
    }
    pub fn page_size(&self) -> u32 {
        self.header.database_page_size
    }
}
