#![allow(unused, dead_code)]

mod bytes;
pub mod errors;
pub mod format;
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
        self.validate_page(page_no, None::<fn(_) -> bool>)?;
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

    pub fn read_raw_page_into<B: AsMut<[u8]> + ?Sized>(
        &mut self,
        page_no: PageNumber,
        buff: &mut B,
    ) -> Result<(), SqliteDatabaseError> {
        if page_no == 1 {
            return Err(SqliteDatabaseError::Corrupt(
                "Page no '1' cant be used as raw page".into(),
            ));
        }
        self.validate_page(page_no, None::<fn(_) -> bool>);
        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        let buff = buff.as_mut();
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
        F: Fn(PageNumber) -> bool,
    {
        if let Some(exc) = exception {
            if exc(page_no) {
                return Err(SqliteDatabaseError::Corrupt("Exception Failed".into()));
            }
        }
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

    pub fn page_count(&self) -> u32 {
        self.header.database_size_in_pages
    }
    pub fn page_size(&self) -> u32 {
        self.header.database_page_size
    }
}
