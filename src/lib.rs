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
        if page_no == 0 {
            return Err(SqliteDatabaseError::Corrupt(
                "page number cannot be zero".into(),
            ));
        }
        if page_no > self.header.database_size_in_pages {
            return Err(SqliteDatabaseError::Corrupt(
                "page number is outside the database".into(),
            ));
        }

        let page_size = self.header.database_page_size;
        let offset = page_size * (page_no - 1);
        self.file.seek(SeekFrom::Start(offset as u64))?;

        let mut buff = vec![0u8; page_size as usize];
        self.file.read_exact(&mut buff)?;

        BTreePage::parse(
            buff,
            page_no,
            (page_size - self.header.reserved_space as u32) as u16,
        )
    }
}
