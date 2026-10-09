#![allow(unused)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;

use crate::InkResult;
use crate::errors::InkError;
use crate::util::assert_one;
use crate::vfs::cursor::FileCursor;
use crate::vfs::file::InkFile;

use self::DbFormat::Ink;

use super::MemCursor;

// A database file starts with a header occupying the first bytes of page one.
// Every field below gets an offset constant, counted in bytes from the start of
// that page, and the sizes of the fields are constants of their own. Two headers
// live side by side here: the hundred byte one SQLite defined, and the shorter
// one Ink writes for its own .inkdb files. The names say which is which.
//
// Further Reading: https://sqlite.org/fileformat.html
//
//
/// How many bytes the SQLite header takes at the start of page one.
pub const HEADER_SIZE: u8 = 100;

/// The sixteen bytes a SQLite file opens with. Any file that starts with these
/// is a SQLite database as far as this engine is concerned.
pub const SQLITE_FILE_MAGIC: &[u8; SQLITE_HEADER_STRING_SIZE] = b"SQLite format 3\0";

pub const SQLITE_HEADER_STRING_OFFSET: usize = 0;
pub const SQLITE_HEADER_STRING_SIZE: usize = 16;

pub const SQLITE_DATABASE_PAGE_SIZE_OFFSET: usize = 16;
pub const SQLITE_DATABASE_PAGE_SIZE: usize = 2;

pub const SQLITE_FILE_FORMAT_WRITE_VERSION_OFFSET: usize = 18;
pub const SQLITE_FILE_FORMAT_WRITE_VERSION_SIZE: usize = 1;

pub const SQLITE_FILE_FORMAT_READ_VERSION_OFFSET: usize = 19;
pub const SQLITE_FILE_FORMAT_READ_VERSION_SIZE: usize = 1;

pub const SQLITE_RESERVED_SPACE_OFFSET: usize = 20;
pub const SQLITE_RESERVED_SPACE_SIZE: usize = 1;

pub const SQLITE_MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET: usize = 21;
pub const SQLITE_MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const SQLITE_MINIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET: usize = 22;
pub const SQLITE_MINIMUM_EMBEDDED_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const SQLITE_LEAF_PAYLOAD_FRACTION_OFFSET: usize = 23;
pub const SQLITE_LEAF_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const SQLITE_FILE_CHANGE_COUNTER_OFFSET: usize = 24;
pub const SQLITE_FILE_CHANGE_COUNTER_SIZE: usize = 4;

pub const SQLITE_DATABASE_SIZE_IN_PAGES_OFFSET: usize = 28;
pub const SQLITE_DATABASE_SIZE_IN_PAGES_SIZE: usize = 4;

pub const SQLITE_FIRST_FREELIST_TRUNK_PAGE_OFFSET: usize = 32;
pub const SQLITE_FIRST_FREELIST_TRUNK_PAGE_SIZE: usize = 4;

pub const SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET: usize = 36;
pub const SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE: usize = 4;

pub const SQLITE_SCHEMA_COOKIE_OFFSET: usize = 40;
pub const SQLITE_SCHEMA_COOKIE_SIZE: usize = 4;

pub const SQLITE_SCHEMA_FORMAT_NUMBER_OFFSET: usize = 44;
pub const SQLITE_SCHEMA_FORMAT_NUMBER_SIZE: usize = 4;

pub const SQLITE_DEFAULT_PAGE_CACHE_SIZE_OFFSET: usize = 48;
pub const SQLITE_DEFAULT_PAGE_CACHE_SIZE: usize = 4;

pub const SQLITE_LARGEST_ROOT_BTREE_PAGE_OFFSET: usize = 52;
pub const SQLITE_LARGEST_ROOT_BTREE_PAGE_SIZE: usize = 4;

pub const SQLITE_DATABASE_TEXT_ENCODING_OFFSET: usize = 56;
pub const SQLITE_DATABASE_TEXT_ENCODING_SIZE: usize = 4;

pub const SQLITE_USER_VERSION_OFFSET: usize = 60;
pub const SQLITE_USER_VERSION_SIZE: usize = 4;

pub const SQLITE_INCREMENTAL_VACUUM_MODE_OFFSET: usize = 64;
pub const SQLITE_INCREMENTAL_VACUUM_MODE_SIZE: usize = 4;

pub const SQLITE_APPLICATION_ID_OFFSET: usize = 68;
pub const SQLITE_APPLICATION_ID_SIZE: usize = 4;

pub const SQLITE_RESERVED_FOR_EXPANSION_OFFSET: usize = 72;
pub const SQLITE_RESERVED_FOR_EXPANSION_SIZE: usize = 20;

pub const SQLITE_VERSION_VALID_FOR_NUMBER_OFFSET: usize = 92;
pub const SQLITE_VERSION_VALID_FOR_NUMBER_SIZE: usize = 4;

pub const SQLITE_VERSION_NUMBER_OFFSET: usize = 96;
pub const SQLITE_VERSION_NUMBER_SIZE: usize = 4;

/// What an Ink file opens with, which is how [`DatabaseHeader::detect`] tells
/// the two formats apart.
pub const INK_MAGIC: &[u8; INK_MAGIC_SIZE] = b"InkDB format 1";
/// Length of the bytes above.
pub const INK_MAGIC_SIZE: usize = 14;
/// How many bytes the Ink header takes. Much shorter than the SQLite one,
/// because it only carries the handful of fields this engine actually uses (might be extended in the near future).
pub const INK_HEADER_SIZE: usize = 35;

pub const INK_PAGE_SIZE_OFFSET: usize = 14;
pub const INK_PAGE_SIZE_SIZE: usize = 4;

pub const INK_RESERVED_SPACE_OFFSET: usize = 18;
pub const INK_RESERVED_SPACE_SIZE: usize = 1;

pub const INK_SIZE_IN_PAGES_OFFSET: usize = 19;
pub const INK_SIZE_IN_PAGES_SIZE: usize = 4;

pub const INK_FREELIST_TRUNK_OFFSET: usize = 23;
pub const INK_FREELIST_TRUNK_SIZE: usize = 4;

pub const INK_FREELIST_TOTAL_OFFSET: usize = 27;
pub const INK_FREELIST_TOTAL_SIZE: usize = 4;

pub const INK_VERSION_OFFSET: usize = 31;
pub const INK_VERSION_SIZE: usize = 4;

/// The version number written into a fresh Ink file.
pub const INK_DEFAULT_VERSION: u32 = 1000000;
/// The page size a fresh database is created with, in bytes.
pub const DEFAULT_PAGE_SIZE: u32 = 4096;

/// Which of the two layouts a file is written in.
///
/// `Sqlite` is the format SQLite defined, which this engine both reads and
/// writes so that other tools can open its files. `Ink` is the engine's own,
/// shorter header, used for files whose name ends in `.inkdb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFormat {
    Sqlite,
    Ink,
}

impl DbFormat {
    /// How many bytes of page one this format spends on the header, and so
    /// where the cells of the master table start.
    pub fn header_len(self) -> usize {
        match self {
            DbFormat::Sqlite => HEADER_SIZE as usize,
            DbFormat::Ink => INK_HEADER_SIZE,
        }
    }

    /// Pick a format from the file name: a `.inkdb` extension means Ink, and
    /// anything else means SQLite, so a new file can be opened by the tools
    /// that expect that layout.
    pub fn for_path<P: AsRef<std::path::Path>>(path: P) -> Self {
        match path.as_ref().extension().and_then(|e| e.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("inkdb") => DbFormat::Ink,
            _ => DbFormat::Sqlite,
        }
    }

    /// Where the page count sits in the header, which each format stores in a
    /// different place.
    pub fn size_in_pages_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => SQLITE_DATABASE_SIZE_IN_PAGES_OFFSET,
            DbFormat::Ink => INK_SIZE_IN_PAGES_OFFSET,
        }
    }

    /// Where the first freelist trunk page number sits in the header.
    pub fn freelist_trunk_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => SQLITE_FIRST_FREELIST_TRUNK_PAGE_OFFSET,
            DbFormat::Ink => INK_FREELIST_TRUNK_OFFSET,
        }
    }

    /// Where the number of freelist pages sits in the header.
    pub fn freelist_total_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET,
            DbFormat::Ink => INK_FREELIST_TOTAL_OFFSET,
        }
    }
}

/**
  The header of an Ink file, in the short layout this engine defines.
  It keeps only what the pager and the freelist need: the magic string, the
  page size, the size of the file in pages, the head of the freelist and the
  version. Everything a SQLite header carries but nothing here reads is simpl
  left out.
*/
#[derive(Debug, Clone, Copy)]
pub struct InkFileHeader {
    header_string: [u8; INK_MAGIC_SIZE],
    database_page_size: u32,
    reserved_space: u8,
    database_size_in_pages: u32,
    first_freelist_trunk_page: u32,
    total_number_of_freelist_pages: u32,
    version_number: u32,
}

impl InkFileHeader {
    pub(crate) fn database_page_size(&self) -> u32 {
        self.database_page_size
    }

    pub(crate) fn reserved_space(&self) -> u8 {
        self.reserved_space
    }

    pub(crate) fn database_size_in_pages(&self) -> u32 {
        self.database_size_in_pages
    }

    pub(crate) fn first_freelist_trunk_page(&self) -> u32 {
        self.first_freelist_trunk_page
    }

    pub(crate) fn total_number_of_freelist_pages(&self) -> u32 {
        self.total_number_of_freelist_pages
    }
}

impl Default for InkFileHeader {
    /// The header of a brand new Ink file: default page size, one page long,
    /// and an empty freelist.
    fn default() -> Self {
        Self {
            header_string: *INK_MAGIC,
            database_page_size: DEFAULT_PAGE_SIZE,
            reserved_space: 0,
            database_size_in_pages: 1,
            first_freelist_trunk_page: 0,
            total_number_of_freelist_pages: 0,
            version_number: INK_DEFAULT_VERSION,
        }
    }
}

impl InkFileHeader {
    /// Read an Ink header from the start of `source`.
    ///
    /// # Errors
    /// [`InkError::InvalidDatabaseHeader`] when the file is too short to hold a
    /// header, or when the values in it do not make sense.
    pub fn parse<R: InkFile>(source: &'_ R) -> Result<Self, InkError> {
        let mut raw = [0u8; INK_HEADER_SIZE];
        source
            .read_exact_at(0, &mut raw)
            .map_err(|_| InkError::InvalidDatabaseHeader)?;
        let header = Self {
            header_string: raw[0..INK_MAGIC_SIZE]
                .try_into()
                .map_err(|_| InkError::InvalidDatabaseHeader)?,
            database_page_size: u32::from_be_bytes(
                raw[INK_PAGE_SIZE_OFFSET..INK_PAGE_SIZE_OFFSET + INK_PAGE_SIZE_SIZE]
                    .try_into()
                    .map_err(|_| InkError::InvalidDatabaseHeader)?,
            ),
            reserved_space: raw[INK_RESERVED_SPACE_OFFSET],
            database_size_in_pages: u32::from_be_bytes(
                raw[INK_SIZE_IN_PAGES_OFFSET..INK_SIZE_IN_PAGES_OFFSET + INK_SIZE_IN_PAGES_SIZE]
                    .try_into()
                    .map_err(|_| InkError::InvalidDatabaseHeader)?,
            ),
            first_freelist_trunk_page: u32::from_be_bytes(
                raw[INK_FREELIST_TRUNK_OFFSET..INK_FREELIST_TRUNK_OFFSET + INK_FREELIST_TRUNK_SIZE]
                    .try_into()
                    .map_err(|_| InkError::InvalidDatabaseHeader)?,
            ),
            total_number_of_freelist_pages: u32::from_be_bytes(
                raw[INK_FREELIST_TOTAL_OFFSET..INK_FREELIST_TOTAL_OFFSET + INK_FREELIST_TOTAL_SIZE]
                    .try_into()
                    .map_err(|_| InkError::InvalidDatabaseHeader)?,
            ),
            version_number: u32::from_be_bytes(
                raw[INK_VERSION_OFFSET..INK_VERSION_OFFSET + INK_VERSION_SIZE]
                    .try_into()
                    .map_err(|_| InkError::InvalidDatabaseHeader)?,
            ),
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(&self) -> Result<(), InkError> {
        if self.header_string != *INK_MAGIC {
            return Err(InkError::InvalidDatabaseHeader);
        }
        if self.database_page_size != 1
            && (self.database_page_size < 512
                || self.database_page_size > 32768
                || !self.database_page_size.is_power_of_two())
        {
            return Err(InkError::InvalidPageSize(self.database_page_size as u16));
        }
        if (self.reserved_space as u32) >= self.page_size_real() {
            return Err(InkError::InvalidDatabaseHeader);
        }
        Ok(())
    }
    /**
        The database page size in bytes. Must be a power of two
        between 512 and 32768 inclusive, or the value 1
        representing a page size of 65536.
    */
    fn page_size_real(&self) -> u32 {
        if self.database_page_size == 1 {
            65536
        } else {
            self.database_page_size
        }
    }

    /// Lay the header out as the bytes it is stored as.
    pub fn serialize(&self) -> [u8; INK_HEADER_SIZE] {
        let mut raw = [0u8; INK_HEADER_SIZE];
        raw[0..INK_MAGIC_SIZE].copy_from_slice(&self.header_string);
        raw[INK_PAGE_SIZE_OFFSET..INK_PAGE_SIZE_OFFSET + INK_PAGE_SIZE_SIZE]
            .copy_from_slice(&self.database_page_size.to_be_bytes());
        raw[INK_RESERVED_SPACE_OFFSET] = self.reserved_space;
        raw[INK_SIZE_IN_PAGES_OFFSET..INK_SIZE_IN_PAGES_OFFSET + INK_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&self.database_size_in_pages.to_be_bytes());
        raw[INK_FREELIST_TRUNK_OFFSET..INK_FREELIST_TRUNK_OFFSET + INK_FREELIST_TRUNK_SIZE]
            .copy_from_slice(&self.first_freelist_trunk_page.to_be_bytes());
        raw[INK_FREELIST_TOTAL_OFFSET..INK_FREELIST_TOTAL_OFFSET + INK_FREELIST_TOTAL_SIZE]
            .copy_from_slice(&self.total_number_of_freelist_pages.to_be_bytes());
        raw[INK_VERSION_OFFSET..INK_VERSION_OFFSET + INK_VERSION_SIZE]
            .copy_from_slice(&self.version_number.to_be_bytes());
        raw
    }
}

/// A header, whichever format it was written in.
#[derive(Debug, Clone, Copy)]
pub enum DatabaseHeader {
    Sqlite(SqliteDatabaseHeader),
    Ink(InkFileHeader),
}

impl DatabaseHeader {
    /// Which format this header belongs to.
    pub fn format(&self) -> DbFormat {
        match self {
            DatabaseHeader::Sqlite(_) => DbFormat::Sqlite,
            DatabaseHeader::Ink(_) => DbFormat::Ink,
        }
    }

    /// How many bytes of page one the header takes.
    pub fn header_len(&self) -> usize {
        self.format().header_len()
    }

    /// The page size in bytes, with the 65536 case already handled.
    pub fn page_size(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.database_page_size,
            DatabaseHeader::Ink(h) => {
                if h.database_page_size == 1 {
                    65536
                } else {
                    h.database_page_size
                }
            }
        }
    }

    /// Bytes at the end of every page that the format keeps for itself. They sit
    /// outside the usable area, so cells are never placed there.
    pub fn reserved_space(&self) -> u8 {
        match self {
            DatabaseHeader::Sqlite(h) => h.reserved_space,
            DatabaseHeader::Ink(h) => h.reserved_space,
        }
    }

    /// How many bytes of a page are actually available for cells.
    pub fn usable_size(&self) -> u32 {
        self.page_size() - self.reserved_space() as u32
    }

    /// The size of the file in pages.
    pub fn size_in_pages(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.database_size_in_pages,
            DatabaseHeader::Ink(h) => h.database_size_in_pages,
        }
    }

    /// Page number of the first freelist trunk, or zero when there is no free
    /// page yet.
    pub fn freelist_trunk(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.first_freelist_trunk_page,
            DatabaseHeader::Ink(h) => h.first_freelist_trunk_page,
        }
    }

    /// How many pages the freelist holds, which must match what walking the
    /// trunk chain finds.
    pub fn freelist_total(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.total_number_of_freelist_pages,
            DatabaseHeader::Ink(h) => h.total_number_of_freelist_pages,
        }
    }

    /// Work out which format the file is by looking at the bytes it starts with.
    ///
    /// # Errors
    /// [`InkError::InvalidDatabaseHeader`] when neither magic string matches,
    /// which means this is not a database this engine can open.
    pub fn detect<R: InkFile>(source: &'_ R) -> InkResult<DbFormat> {
        // This one is safe for INK_MAGIC_HEADER since
        // SQLITE_HEADER_STRING_SIZE > INK_MAGIC_HEADER
        let mut magic = [0u8; SQLITE_HEADER_STRING_SIZE];
        source
            .read_exact_at(0, &mut magic)
            .map_err(|_| InkError::InvalidDatabaseHeader)?;
        if magic == *SQLITE_FILE_MAGIC {
            return Ok(DbFormat::Sqlite);
        }
        if magic[..INK_MAGIC_SIZE] == *INK_MAGIC {
            return Ok(DbFormat::Ink);
        }
        Err(InkError::InvalidDatabaseHeader)
    }

    /// Read the header of `source`, in whichever format it turns out to be.
    ///
    /// # Errors
    /// Whatever [`DatabaseHeader::detect`] and the format's own parser reject,
    /// so a corrupt or unknown file fails here before anything else runs.
    pub fn parse<R: InkFile>(source: &'_ R) -> Result<Self, InkError> {
        match Self::detect(source)? {
            DbFormat::Sqlite => Ok(DatabaseHeader::Sqlite(SqliteDatabaseHeader::parse(source)?)),
            DbFormat::Ink => Ok(DatabaseHeader::Ink(InkFileHeader::parse(source)?)),
        }
    }

    /// A header for a database that is about to be created, filled in with the
    /// defaults for that format.
    pub fn default_for(format: DbFormat) -> Self {
        match format {
            DbFormat::Sqlite => DatabaseHeader::Sqlite(SqliteDatabaseHeader::new_database()),
            DbFormat::Ink => DatabaseHeader::Ink(InkFileHeader::default()),
        }
    }

    /// The header as the bytes it is stored as.
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            DatabaseHeader::Sqlite(h) => h.serialize().to_vec(),
            DatabaseHeader::Ink(h) => h.serialize().to_vec(),
        }
    }
}
/// The header of a SQLite file, field for field.
///
/// The engine reads this layout because most databases it meets were written by
/// SQLite, and it writes the same layout when it creates a file, so both sides
/// can open the result.
///
// Again for more information about the format, see:
// https://sqlite.org/fileformat.html
#[derive(Debug, Clone, Copy)]
pub struct SqliteDatabaseHeader {
    // The header string: "SQLite format 3\000"
    header_string: [u8; 16],

    // The database page size in bytes.
    database_page_size: u32,

    // File format write version. 1 for legacy; 2 for WAL.
    file_format_write_version: u8,

    // File format read version. 1 for legacy; 2 for WAL.
    file_format_read_version: u8, /* Unused by the engine after creation */

    // Bytes of unused "reserved" space at the end of each page. Usually 0.
    reserved_space: u8, /* Unused by the engine after creation */

    // Maximum embedded payload fraction. Must be 64.
    maximum_embedded_payload_fraction: u8, /* Unused by the engine after creation */

    // Minimum embedded payload fraction. Must be 32.
    minimum_embedded_payload_fraction: u8, /* Unused by the engine after creation */

    // Leaf payload fraction. Must be 32.
    leaf_payload_fraction: u8, /* Unused by the engine after creation */

    // File change counter.
    file_change_counter: u32, /* Unused by the engine after creation */

    // Size of the database file in pages.
    database_size_in_pages: u32,

    // Page number of the first freelist trunk page.
    first_freelist_trunk_page: u32,

    // Total number of freelist pages.
    total_number_of_freelist_pages: u32,

    // The schema cookie.
    schema_cookie: u32, /* Unused by the engine after creation */

    // The schema format number. Supported schema formats are 1, 2, 3, and 4.
    schema_format_number: u32, /* Unused by the engine after creation */

    // Default page cache size.
    default_page_cache_size: u32,

    // The page number of the largest root b-tree page when
    // in auto-vacuum or incremental-vacuum modes, or zero otherwise.
    largest_root_btree_page: u32, /* Unused by the engine after creation */

    // The database text encoding.
    // A value of 1 means UTF-8.
    // A value of 2 means UTF-16le.
    // A value of 3 means UTF-16be.
    database_text_encoding: u32,

    user_version: u32, /* Unused by the engine after creation */

    // True (non-zero) for incremental-vacuum mode. False (zero) otherwise.
    incremental_vacuum_mode: u32, /* Unused by the engine after creation */

    application_id: u32, /* Unused by the engine after creation */

    // Reserved for expansion. Must be zero.
    reserved_for_expansion: [u8; 20], /* Unused by the engine after creation */

    version_valid_for_number: u32, /* Unused by the engine after creation */

    version_number: u32, /* Unused by the engine after creation */
}

impl SqliteDatabaseHeader {
    pub(crate) fn database_page_size(&self) -> u32 {
        self.database_page_size
    }

    pub(crate) fn reserved_space(&self) -> u8 {
        self.reserved_space
    }

    pub(crate) fn database_size_in_pages(&self) -> u32 {
        self.database_size_in_pages
    }

    pub(crate) fn first_freelist_trunk_page(&self) -> u32 {
        self.first_freelist_trunk_page
    }

    pub(crate) fn total_number_of_freelist_pages(&self) -> u32 {
        self.total_number_of_freelist_pages
    }
}

impl SqliteDatabaseHeader {
    /// The error these checks all report, kept short because it is used a lot
    /// below.
    const INVALID_HEADER_ERR: InkError = InkError::InvalidDatabaseHeader;

    /// Read the SQLite header, then check that every field in it holds a value
    /// the format allows.
    ///
    /// The fields are read in the order they are stored, which is what lets one
    /// cursor walk the whole header.
    ///
    /// # Errors
    /// [`InkError::InvalidDatabaseHeader`] when the magic string is wrong or a
    /// field holds something impossible, and [`InkError::InvalidPageSize`] when
    /// the page size is not one of the allowed ones.
    pub fn parse<R: InkFile>(source: &'_ R) -> Result<Self, InkError> {
        let mut cursor = FileCursor::<'_, R>::new(source);

        let header_string = cursor.read_next_array::<SQLITE_HEADER_STRING_SIZE>()?;
        let database_page_size = cursor.read_next_u16()?;
        let file_format_write_version = cursor.read_next_u8()?;
        let file_format_read_version = cursor.read_next_u8()?;
        let reserved_space = cursor.read_next_u8()?;
        let maximum_embedded_payload_fraction = cursor.read_next_u8()?;
        let minimum_embedded_payload_fraction = cursor.read_next_u8()?;
        let leaf_payload_fraction = cursor.read_next_u8()?;
        let file_change_counter = cursor.read_next_u32()?;
        let database_size_in_pages = cursor.read_next_u32()?;
        let first_freelist_trunk_page = cursor.read_next_u32()?;
        let total_number_of_freelist_pages = cursor.read_next_u32()?;
        let schema_cookie = cursor.read_next_u32()?;
        let schema_format_number = cursor.read_next_u32()?;
        let default_page_cache_size = cursor.read_next_u32()?;
        let largest_root_btree_page = cursor.read_next_u32()?;
        let database_text_encoding = cursor.read_next_u32()?;
        let user_version = cursor.read_next_u32()?;
        let incremental_vacuum_mode = cursor.read_next_u32()?;
        let application_id = cursor.read_next_u32()?;
        let reserved_for_expansion =
            cursor.read_next_array::<SQLITE_RESERVED_FOR_EXPANSION_SIZE>()?;
        let version_valid_for_number = cursor.read_next_u32()?;
        let version_number = cursor.read_next_u32()?;
        let mut header = Self {
            header_string,
            database_page_size: database_page_size as u32,
            file_format_write_version,
            file_format_read_version,
            reserved_space,
            maximum_embedded_payload_fraction,
            minimum_embedded_payload_fraction,
            leaf_payload_fraction,
            file_change_counter,
            database_size_in_pages,
            first_freelist_trunk_page,
            total_number_of_freelist_pages,
            schema_cookie,
            schema_format_number,
            default_page_cache_size,
            largest_root_btree_page,
            database_text_encoding,
            user_version,
            incremental_vacuum_mode,
            application_id,
            reserved_for_expansion,
            version_valid_for_number,
            version_number,
        };
        header.validate()?;

        Ok(header)
    }

    /// Check the fields a SQLite file is required to agree on, and normalise the
    /// page size on the way through.
    ///
    /// These are the values SQLite itself refuses to open a file without, so a
    /// file that passes here is one the other tools will accept too.
    ///
    /// # Errors
    /// [`InkError::InvalidDatabaseHeader`] when a field holds a value outside
    /// the range the format allows, and [`InkError::InvalidPageSize`] when the
    /// page size is not a power of two between 512 and 32768.
    ///
    /// All validation steps are from [The Database Header](https://sqlite.org/fileformat.html#the_database_header).
    fn validate(&mut self) -> Result<(), InkError> {
        let Self {
            header_string,
            database_page_size,
            file_format_write_version,
            file_format_read_version,
            reserved_space,
            maximum_embedded_payload_fraction,
            minimum_embedded_payload_fraction,
            leaf_payload_fraction,
            file_change_counter,
            database_size_in_pages,
            first_freelist_trunk_page,
            total_number_of_freelist_pages,
            schema_cookie,
            schema_format_number,
            default_page_cache_size,
            largest_root_btree_page,
            database_text_encoding,
            user_version,
            incremental_vacuum_mode,
            application_id,
            reserved_for_expansion,
            version_valid_for_number,
            version_number,
        } = self;

        assert_one(
            self.header_string == *SQLITE_FILE_MAGIC,
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            self.database_page_size == 1
                || (self.database_page_size >= 512
                    && self.database_page_size <= 32768
                    && self.database_page_size.is_power_of_two()),
            InkError::InvalidPageSize(self.database_page_size as u16),
        )?;

        if self.database_page_size == 1 {
            self.database_page_size = 65536;
        }

        assert_one(
            matches!(self.file_format_write_version, 1 | 2),
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            matches!(self.file_format_read_version, 1 | 2),
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            (self.reserved_space as u32) < self.database_page_size,
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            self.maximum_embedded_payload_fraction == 64,
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            self.minimum_embedded_payload_fraction == 32,
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(self.leaf_payload_fraction == 32, Self::INVALID_HEADER_ERR)?;

        assert_one(
            matches!(self.schema_format_number, 1..=4),
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            matches!(self.database_text_encoding, 1..=3),
            Self::INVALID_HEADER_ERR,
        )?;

        assert_one(
            self.reserved_for_expansion.iter().all(|&byte| byte == 0),
            Self::INVALID_HEADER_ERR,
        )?;
        Ok(())
    }

    /// The header of a database that is about to be created: the values SQLite
    /// writes when it makes a new file.
    // NOTE: The reserved_space value is not deterministic, so we leave it as 0 for now.
    pub fn new_database() -> Self {
        Self {
            header_string: *SQLITE_FILE_MAGIC,
            database_page_size: DEFAULT_PAGE_SIZE,
            file_format_write_version: 1,
            file_format_read_version: 1,
            reserved_space: 0,
            maximum_embedded_payload_fraction: 64,
            minimum_embedded_payload_fraction: 32,
            leaf_payload_fraction: 32,
            file_change_counter: 1,
            database_size_in_pages: 1,
            first_freelist_trunk_page: 0,
            total_number_of_freelist_pages: 0,
            schema_cookie: 1,
            schema_format_number: 4,
            default_page_cache_size: 0,
            largest_root_btree_page: 0,
            database_text_encoding: 1,
            user_version: 0,
            incremental_vacuum_mode: 0,
            application_id: 0,
            reserved_for_expansion: [0u8; 20],
            version_valid_for_number: 1,
            version_number: 3043002,
        }
    }

    /// Lay the header out as the bytes it is stored as.
    pub fn serialize(&self) -> [u8; HEADER_SIZE as usize] {
        let mut raw = [0u8; HEADER_SIZE as usize];
        raw[SQLITE_HEADER_STRING_OFFSET..SQLITE_HEADER_STRING_OFFSET + SQLITE_HEADER_STRING_SIZE]
            .copy_from_slice(&self.header_string);
        raw[SQLITE_DATABASE_PAGE_SIZE_OFFSET
            ..SQLITE_DATABASE_PAGE_SIZE_OFFSET + SQLITE_DATABASE_PAGE_SIZE]
            .copy_from_slice(&(self.database_page_size as u16).to_be_bytes());
        raw[SQLITE_FILE_FORMAT_WRITE_VERSION_OFFSET] = self.file_format_write_version;
        raw[SQLITE_FILE_FORMAT_READ_VERSION_OFFSET] = self.file_format_read_version;
        raw[SQLITE_RESERVED_SPACE_OFFSET] = self.reserved_space;
        raw[SQLITE_MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET] =
            self.maximum_embedded_payload_fraction;
        raw[SQLITE_MINIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET] =
            self.minimum_embedded_payload_fraction;
        raw[SQLITE_LEAF_PAYLOAD_FRACTION_OFFSET] = self.leaf_payload_fraction;
        raw[SQLITE_FILE_CHANGE_COUNTER_OFFSET
            ..SQLITE_FILE_CHANGE_COUNTER_OFFSET + SQLITE_FILE_CHANGE_COUNTER_SIZE]
            .copy_from_slice(&self.file_change_counter.to_be_bytes());
        raw[SQLITE_DATABASE_SIZE_IN_PAGES_OFFSET
            ..SQLITE_DATABASE_SIZE_IN_PAGES_OFFSET + SQLITE_DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&self.database_size_in_pages.to_be_bytes());
        raw[SQLITE_FIRST_FREELIST_TRUNK_PAGE_OFFSET
            ..SQLITE_FIRST_FREELIST_TRUNK_PAGE_OFFSET + SQLITE_FIRST_FREELIST_TRUNK_PAGE_SIZE]
            .copy_from_slice(&self.first_freelist_trunk_page.to_be_bytes());
        raw[SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET
            ..SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET
                + SQLITE_TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE]
            .copy_from_slice(&self.total_number_of_freelist_pages.to_be_bytes());
        raw[SQLITE_SCHEMA_COOKIE_OFFSET..SQLITE_SCHEMA_COOKIE_OFFSET + SQLITE_SCHEMA_COOKIE_SIZE]
            .copy_from_slice(&self.schema_cookie.to_be_bytes());
        raw[SQLITE_SCHEMA_FORMAT_NUMBER_OFFSET
            ..SQLITE_SCHEMA_FORMAT_NUMBER_OFFSET + SQLITE_SCHEMA_FORMAT_NUMBER_SIZE]
            .copy_from_slice(&self.schema_format_number.to_be_bytes());
        raw[SQLITE_DEFAULT_PAGE_CACHE_SIZE_OFFSET
            ..SQLITE_DEFAULT_PAGE_CACHE_SIZE_OFFSET + SQLITE_DEFAULT_PAGE_CACHE_SIZE]
            .copy_from_slice(&self.default_page_cache_size.to_be_bytes());
        raw[SQLITE_LARGEST_ROOT_BTREE_PAGE_OFFSET
            ..SQLITE_LARGEST_ROOT_BTREE_PAGE_OFFSET + SQLITE_LARGEST_ROOT_BTREE_PAGE_SIZE]
            .copy_from_slice(&self.largest_root_btree_page.to_be_bytes());
        raw[SQLITE_DATABASE_TEXT_ENCODING_OFFSET
            ..SQLITE_DATABASE_TEXT_ENCODING_OFFSET + SQLITE_DATABASE_TEXT_ENCODING_SIZE]
            .copy_from_slice(&self.database_text_encoding.to_be_bytes());
        raw[SQLITE_USER_VERSION_OFFSET..SQLITE_USER_VERSION_OFFSET + SQLITE_USER_VERSION_SIZE]
            .copy_from_slice(&self.user_version.to_be_bytes());
        raw[SQLITE_INCREMENTAL_VACUUM_MODE_OFFSET
            ..SQLITE_INCREMENTAL_VACUUM_MODE_OFFSET + SQLITE_INCREMENTAL_VACUUM_MODE_SIZE]
            .copy_from_slice(&self.incremental_vacuum_mode.to_be_bytes());
        raw[SQLITE_APPLICATION_ID_OFFSET
            ..SQLITE_APPLICATION_ID_OFFSET + SQLITE_APPLICATION_ID_SIZE]
            .copy_from_slice(&self.application_id.to_be_bytes());
        raw[SQLITE_RESERVED_FOR_EXPANSION_OFFSET
            ..SQLITE_RESERVED_FOR_EXPANSION_OFFSET + SQLITE_RESERVED_FOR_EXPANSION_SIZE]
            .copy_from_slice(&self.reserved_for_expansion);
        raw[SQLITE_VERSION_VALID_FOR_NUMBER_OFFSET
            ..SQLITE_VERSION_VALID_FOR_NUMBER_OFFSET + SQLITE_VERSION_VALID_FOR_NUMBER_SIZE]
            .copy_from_slice(&self.version_valid_for_number.to_be_bytes());
        raw[SQLITE_VERSION_NUMBER_OFFSET
            ..SQLITE_VERSION_NUMBER_OFFSET + SQLITE_VERSION_NUMBER_SIZE]
            .copy_from_slice(&self.version_number.to_be_bytes());
        raw
    }
}
