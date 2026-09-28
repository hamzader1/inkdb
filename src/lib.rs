#![allow(unused, dead_code)] // temp for now
// #![warn(unused_results)]

pub mod backend;
mod bytes;
pub mod db;
use crate::db::header::InkDatabaseHeader;
pub mod record;
mod schema;
pub mod shell;
pub mod sql;
pub use schema::Master;
pub mod errors;

mod macros;
pub mod pager;
pub mod storage;
mod util;

pub mod varint;
pub mod vfs;
use errors::InkError;

pub use storage::cursor::MemCursor;
pub type DbError = InkError;

pub type Result<T, E = InkError> = std::result::Result<T, E>;

use self::vfs::Vfs;
pub type InkResult<T> = Result<T, InkError>;
use crate::pager::pager::Pager;

pub struct InkDatabase<V: Vfs> {
    pub pager: Pager<V>,
    header: InkDatabaseHeader,
}

// impl InkDatabase<DiskFile> {
//     pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, InkError> {
//         let default_vfs = DiskVfs;
//         Self::with_source(default_vfs, db_path)
//     }
//     pub fn with_cache<P: AsRef<Path>>(db_path: P, cache_size: usize) -> Result<Self, InkError> {
//         let default_vfs = DiskVfs;
//         Self::with_source_cache(default_vfs, db_path, cache_size)
//     }
// }

// // 'f file source
// impl<F: InkFile> InkDatabase<F> {
//     pub fn with_source<P: AsRef<Path>, V>(mut vfs: V, path: P) -> Result<Self, InkError>
//     where
//         V: Vfs<File = F>,
//     {
//         let source = vfs.open(path, InkOptions::default())?;
//         let header = InkDatabaseHeader::parse(&source)?;
//         let pager = Pager::new(
//             source,
//             header.database_page_size as _,
//             (header.database_page_size - header.reserved_space as u32) as _,
//             header.database_size_in_pages as _,
//         )?;
//         Ok(Self { pager, header })
//     }
//     pub fn with_source_cache<P: AsRef<Path>, V>(
//         mut vfs: V,
//         path: P,
//         cache_size: usize,
//     ) -> Result<Self, InkError>
//     where
//         V: Vfs<File = F>,
//     {
//         let source = vfs.open(path, InkOptions::default())?;
//         let header = InkDatabaseHeader::parse(&source)?;
//         let pager = Pager::with_cache(
//             source,
//             header.database_page_size as _,
//             (header.database_page_size - header.reserved_space as u32) as _,
//             header.database_size_in_pages as _,
//             cache_size,
//         )?;
//         Ok(Self { pager, header })
//     }
//     fn source(&self) -> &F {
//         &self.pager.source
//     }
//     fn cursor(&self) -> FileCursor<'_, F> {
//         FileCursor::new(&self.pager.source)
//     }
//     fn cursor_at_offset(&self, offset: u64) -> FileCursor<'_, F> {
//         FileCursor::with_offset(&self.pager.source, offset)
//     }
//     pub fn header(&self) -> &'_ InkDatabaseHeader {
//         &self.header
//     }

//     pub fn usable_size(&self) -> u32 {
//         self.header.database_page_size - self.header.reserved_space as u32
//     }

//     pub fn read_raw_page_into<B: AsMut<[u8]> + ?Sized>(
//         &mut self,
//         page_no: PageNo,
//         buff: &mut B,
//     ) -> Result<(), InkError> {
//         if page_no == 1 {
//             return Err(InkError::Corrupt(
//                 "Page no '1' cant be used as raw page".into(),
//             ));
//         }
//         self.validate_page(page_no, None::<fn(_) -> bool>)?;
//         let page_size = self.header.database_page_size;
//         let offset = page_size * (page_no - 1);
//         let buff = buff.as_mut();
//         // self.file.seek(SeekFrom::Start(offset as u64))?;

//         self.pager.source.read_exact_at(offset as _, buff)?;

//         Ok(())
//     }

//     fn validate_page<E>(&mut self, page_no: PageNo, exception: Option<E>) -> Result<(), InkError>
//     where
//         E: Fn(PageNo) -> bool,
//     {
//         if let Some(exc) = exception
//             && exc(page_no)
//         {
//             return Err(InkError::InternalFmt(format!(
//                 "page guard exception rejected page {page_no}"
//             )));
//         }
//         if page_no == 0 || page_no > self.header.database_size_in_pages {
//             return Err(InkError::InvalidPageNumber(page_no));
//         }

//         Ok(())
//     }

//     pub fn page_count(&self) -> u32 {
//         self.header.database_size_in_pages
//     }
//     pub fn page_size(&self) -> u32 {
//         self.header.database_page_size
//     }
// }
