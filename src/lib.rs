#![allow(warnings)]
mod bytes;
mod errors;
mod format;
mod macros;
mod util;
use errors::SqliteDatabaseError;
use format::header::SqliteDatabaseHeader;
use format::page::BTreePage;
pub use format::varint::{decode_varint, encode_varint};
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

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

    pub fn page(&mut self, page_no: u32) -> Result<BTreePage, SqliteDatabaseError> {
        let mut bytes_to_read: u32 = {
            if self.header.database_page_size == 1 {
                (u16::MAX as u32) + 1u32
            } else {
                self.header.database_page_size as _
            }
        };
        let offset;
        if page_no == 1 {
            offset = 100;
            bytes_to_read -= 100;
        } else {
            offset = ((bytes_to_read * page_no) - bytes_to_read) as u64;
        }
        self.file.seek(SeekFrom::Start(offset));

        let mut buff = vec![0u8; bytes_to_read as usize];
        self.file.read_exact(&mut buff);
        let mut cur = Cursor::new(&mut buff);

        BTreePage::parse::<Cursor<&mut Vec<u8>>>(&mut cur)
    }
}
