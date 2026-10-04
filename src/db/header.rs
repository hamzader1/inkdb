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

pub const HEADER_SIZE: u8 = 100;

pub const FILE_MAGIC: &[u8; HEADER_STRING_SIZE] = b"SQLite format 3\0";

pub const HEADER_STRING_OFFSET: usize = 0;
pub const HEADER_STRING_SIZE: usize = 16;

pub const DATABASE_PAGE_SIZE_OFFSET: usize = 16;
pub const DATABASE_PAGE_SIZE: usize = 2;

pub const FILE_FORMAT_WRITE_VERSION_OFFSET: usize = 18;
pub const FILE_FORMAT_WRITE_VERSION_SIZE: usize = 1;

pub const FILE_FORMAT_READ_VERSION_OFFSET: usize = 19;
pub const FILE_FORMAT_READ_VERSION_SIZE: usize = 1;

pub const RESERVED_SPACE_OFFSET: usize = 20;
pub const RESERVED_SPACE_SIZE: usize = 1;

pub const MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET: usize = 21;
pub const MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const MINIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET: usize = 22;
pub const MINIMUM_EMBEDDED_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const LEAF_PAYLOAD_FRACTION_OFFSET: usize = 23;
pub const LEAF_PAYLOAD_FRACTION_SIZE: usize = 1;

pub const FILE_CHANGE_COUNTER_OFFSET: usize = 24;
pub const FILE_CHANGE_COUNTER_SIZE: usize = 4;

pub const DATABASE_SIZE_IN_PAGES_OFFSET: usize = 28;
pub const DATABASE_SIZE_IN_PAGES_SIZE: usize = 4;

pub const FIRST_FREELIST_TRUNK_PAGE_OFFSET: usize = 32;
pub const FIRST_FREELIST_TRUNK_PAGE_SIZE: usize = 4;

pub const TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET: usize = 36;
pub const TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE: usize = 4;

pub const SCHEMA_COOKIE_OFFSET: usize = 40;
pub const SCHEMA_COOKIE_SIZE: usize = 4;

pub const SCHEMA_FORMAT_NUMBER_OFFSET: usize = 44;
pub const SCHEMA_FORMAT_NUMBER_SIZE: usize = 4;

pub const DEFAULT_PAGE_CACHE_SIZE_OFFSET: usize = 48;
pub const DEFAULT_PAGE_CACHE_SIZE: usize = 4;

pub const LARGEST_ROOT_BTREE_PAGE_OFFSET: usize = 52;
pub const LARGEST_ROOT_BTREE_PAGE_SIZE: usize = 4;

pub const DATABASE_TEXT_ENCODING_OFFSET: usize = 56;
pub const DATABASE_TEXT_ENCODING_SIZE: usize = 4;

pub const USER_VERSION_OFFSET: usize = 60;
pub const USER_VERSION_SIZE: usize = 4;

pub const INCREMENTAL_VACUUM_MODE_OFFSET: usize = 64;
pub const INCREMENTAL_VACUUM_MODE_SIZE: usize = 4;

pub const APPLICATION_ID_OFFSET: usize = 68;
pub const APPLICATION_ID_SIZE: usize = 4;

pub const RESERVED_FOR_EXPANSION_OFFSET: usize = 72;
pub const RESERVED_FOR_EXPANSION_SIZE: usize = 20;

pub const VERSION_VALID_FOR_NUMBER_OFFSET: usize = 92;
pub const VERSION_VALID_FOR_NUMBER_SIZE: usize = 4;

pub const VERSION_NUMBER_OFFSET: usize = 96;
pub const VERSION_NUMBER_SIZE: usize = 4;

pub const INK_MAGIC: &[u8; INK_MAGIC_SIZE] = b"InkDB format 1";
pub const INK_MAGIC_SIZE: usize = 14;
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

pub const INK_DEFAULT_VERSION: u32 = 1000000;
pub const DEFAULT_PAGE_SIZE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFormat {
    Sqlite,
    Ink,
}

impl DbFormat {
    pub fn header_len(self) -> usize {
        match self {
            DbFormat::Sqlite => HEADER_SIZE as usize,
            DbFormat::Ink => INK_HEADER_SIZE,
        }
    }

    pub fn for_path<P: AsRef<std::path::Path>>(path: P) -> Self {
        match path.as_ref().extension().and_then(|e| e.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("inkdb") => DbFormat::Ink,
            _ => DbFormat::Sqlite,
        }
    }

    pub fn size_in_pages_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => DATABASE_SIZE_IN_PAGES_OFFSET,
            DbFormat::Ink => INK_SIZE_IN_PAGES_OFFSET,
        }
    }

    pub fn freelist_trunk_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => FIRST_FREELIST_TRUNK_PAGE_OFFSET,
            DbFormat::Ink => INK_FREELIST_TRUNK_OFFSET,
        }
    }

    pub fn freelist_total_offset(self) -> usize {
        match self {
            DbFormat::Sqlite => TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET,
            DbFormat::Ink => INK_FREELIST_TOTAL_OFFSET,
        }
    }
}

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

    fn page_size_real(&self) -> u32 {
        if self.database_page_size == 1 {
            65536
        } else {
            self.database_page_size
        }
    }

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

#[derive(Debug, Clone, Copy)]
pub enum DatabaseHeader {
    Sqlite(InkDatabaseHeader),
    Ink(InkFileHeader),
}

impl DatabaseHeader {
    pub fn format(&self) -> DbFormat {
        match self {
            DatabaseHeader::Sqlite(_) => DbFormat::Sqlite,
            DatabaseHeader::Ink(_) => DbFormat::Ink,
        }
    }

    pub fn header_len(&self) -> usize {
        self.format().header_len()
    }

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

    pub fn reserved_space(&self) -> u8 {
        match self {
            DatabaseHeader::Sqlite(h) => h.reserved_space,
            DatabaseHeader::Ink(h) => h.reserved_space,
        }
    }

    pub fn usable_size(&self) -> u32 {
        self.page_size() - self.reserved_space() as u32
    }

    pub fn size_in_pages(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.database_size_in_pages,
            DatabaseHeader::Ink(h) => h.database_size_in_pages,
        }
    }

    pub fn freelist_trunk(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.first_freelist_trunk_page,
            DatabaseHeader::Ink(h) => h.first_freelist_trunk_page,
        }
    }

    pub fn freelist_total(&self) -> u32 {
        match self {
            DatabaseHeader::Sqlite(h) => h.total_number_of_freelist_pages,
            DatabaseHeader::Ink(h) => h.total_number_of_freelist_pages,
        }
    }

    pub fn detect<R: InkFile>(source: &'_ R) -> InkResult<DbFormat> {
        let mut magic = [0u8; HEADER_STRING_SIZE];
        // InkError::InvalidDatabaseHeader
        source
            .read_exact_at(0, &mut magic)
            .map_err(|_| InkError::InvalidDatabaseHeader)?;
        if magic == *FILE_MAGIC {
            return Ok(DbFormat::Sqlite);
        }
        if magic[..INK_MAGIC_SIZE] == *INK_MAGIC {
            return Ok(DbFormat::Ink);
        }
        Err(InkError::InvalidDatabaseHeader)
    }

    pub fn parse<R: InkFile>(source: &'_ R) -> Result<Self, InkError> {
        match Self::detect(source)? {
            DbFormat::Sqlite => Ok(DatabaseHeader::Sqlite(InkDatabaseHeader::parse(source)?)),
            DbFormat::Ink => Ok(DatabaseHeader::Ink(InkFileHeader::parse(source)?)),
        }
    }

    pub fn default_for(format: DbFormat) -> Self {
        match format {
            DbFormat::Sqlite => DatabaseHeader::Sqlite(InkDatabaseHeader::new_database()),
            DbFormat::Ink => DatabaseHeader::Ink(InkFileHeader::default()),
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        match self {
            DatabaseHeader::Sqlite(h) => h.serialize().to_vec(),
            DatabaseHeader::Ink(h) => h.serialize().to_vec(),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct InkDatabaseHeader {
    header_string: [u8; 16],
    database_page_size: u32,
    file_format_write_version: u8,
    file_format_read_version: u8,
    reserved_space: u8,
    maximum_embedded_payload_fraction: u8,
    minimum_embedded_payload_fraction: u8,
    leaf_payload_fraction: u8,
    file_change_counter: u32,
    database_size_in_pages: u32,
    first_freelist_trunk_page: u32,
    total_number_of_freelist_pages: u32,
    schema_cookie: u32,
    schema_format_number: u32,
    default_page_cache_size: u32,
    largest_root_btree_page: u32,
    database_text_encoding: u32,
    user_version: u32,
    incremental_vacuum_mode: u32,
    application_id: u32,
    reserved_for_expansion: [u8; 20],
    version_valid_for_number: u32,
    version_number: u32,
}

impl InkDatabaseHeader {
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

impl InkDatabaseHeader {
    const INVALID_HEADER_ERR: InkError = InkError::InvalidDatabaseHeader;
    pub fn parse<R: InkFile>(source: &'_ R) -> Result<Self, InkError> {
        // default cursor to 0, no manually offset needed
        let mut cursor = FileCursor::<'_, R>::new(source);
        // let mu cursor = MemCursor::new(source.);

        let header_string = cursor.read_next_array::<HEADER_STRING_SIZE>()?;
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
        let reserved_for_expansion = cursor.read_next_array::<RESERVED_FOR_EXPANSION_SIZE>()?;
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

        assert_one(self.header_string == *FILE_MAGIC, Self::INVALID_HEADER_ERR)?;

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

    pub fn new_database() -> Self {
        Self {
            header_string: *FILE_MAGIC,
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

    pub fn serialize(&self) -> [u8; HEADER_SIZE as usize] {
        let mut raw = [0u8; HEADER_SIZE as usize];
        raw[HEADER_STRING_OFFSET..HEADER_STRING_OFFSET + HEADER_STRING_SIZE]
            .copy_from_slice(&self.header_string);
        raw[DATABASE_PAGE_SIZE_OFFSET..DATABASE_PAGE_SIZE_OFFSET + DATABASE_PAGE_SIZE]
            .copy_from_slice(&(self.database_page_size as u16).to_be_bytes());
        raw[FILE_FORMAT_WRITE_VERSION_OFFSET] = self.file_format_write_version;
        raw[FILE_FORMAT_READ_VERSION_OFFSET] = self.file_format_read_version;
        raw[RESERVED_SPACE_OFFSET] = self.reserved_space;
        raw[MAXIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET] = self.maximum_embedded_payload_fraction;
        raw[MINIMUM_EMBEDDED_PAYLOAD_FRACTION_OFFSET] = self.minimum_embedded_payload_fraction;
        raw[LEAF_PAYLOAD_FRACTION_OFFSET] = self.leaf_payload_fraction;
        raw[FILE_CHANGE_COUNTER_OFFSET..FILE_CHANGE_COUNTER_OFFSET + FILE_CHANGE_COUNTER_SIZE]
            .copy_from_slice(&self.file_change_counter.to_be_bytes());
        raw[DATABASE_SIZE_IN_PAGES_OFFSET
            ..DATABASE_SIZE_IN_PAGES_OFFSET + DATABASE_SIZE_IN_PAGES_SIZE]
            .copy_from_slice(&self.database_size_in_pages.to_be_bytes());
        raw[FIRST_FREELIST_TRUNK_PAGE_OFFSET
            ..FIRST_FREELIST_TRUNK_PAGE_OFFSET + FIRST_FREELIST_TRUNK_PAGE_SIZE]
            .copy_from_slice(&self.first_freelist_trunk_page.to_be_bytes());
        raw[TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET
            ..TOTAL_NUMBER_OF_FREELIST_PAGES_OFFSET + TOTAL_NUMBER_OF_FREELIST_PAGES_SIZE]
            .copy_from_slice(&self.total_number_of_freelist_pages.to_be_bytes());
        raw[SCHEMA_COOKIE_OFFSET..SCHEMA_COOKIE_OFFSET + SCHEMA_COOKIE_SIZE]
            .copy_from_slice(&self.schema_cookie.to_be_bytes());
        raw[SCHEMA_FORMAT_NUMBER_OFFSET..SCHEMA_FORMAT_NUMBER_OFFSET + SCHEMA_FORMAT_NUMBER_SIZE]
            .copy_from_slice(&self.schema_format_number.to_be_bytes());
        raw[DEFAULT_PAGE_CACHE_SIZE_OFFSET
            ..DEFAULT_PAGE_CACHE_SIZE_OFFSET + DEFAULT_PAGE_CACHE_SIZE]
            .copy_from_slice(&self.default_page_cache_size.to_be_bytes());
        raw[LARGEST_ROOT_BTREE_PAGE_OFFSET
            ..LARGEST_ROOT_BTREE_PAGE_OFFSET + LARGEST_ROOT_BTREE_PAGE_SIZE]
            .copy_from_slice(&self.largest_root_btree_page.to_be_bytes());
        raw[DATABASE_TEXT_ENCODING_OFFSET
            ..DATABASE_TEXT_ENCODING_OFFSET + DATABASE_TEXT_ENCODING_SIZE]
            .copy_from_slice(&self.database_text_encoding.to_be_bytes());
        raw[USER_VERSION_OFFSET..USER_VERSION_OFFSET + USER_VERSION_SIZE]
            .copy_from_slice(&self.user_version.to_be_bytes());
        raw[INCREMENTAL_VACUUM_MODE_OFFSET
            ..INCREMENTAL_VACUUM_MODE_OFFSET + INCREMENTAL_VACUUM_MODE_SIZE]
            .copy_from_slice(&self.incremental_vacuum_mode.to_be_bytes());
        raw[APPLICATION_ID_OFFSET..APPLICATION_ID_OFFSET + APPLICATION_ID_SIZE]
            .copy_from_slice(&self.application_id.to_be_bytes());
        raw[RESERVED_FOR_EXPANSION_OFFSET
            ..RESERVED_FOR_EXPANSION_OFFSET + RESERVED_FOR_EXPANSION_SIZE]
            .copy_from_slice(&self.reserved_for_expansion);
        raw[VERSION_VALID_FOR_NUMBER_OFFSET
            ..VERSION_VALID_FOR_NUMBER_OFFSET + VERSION_VALID_FOR_NUMBER_SIZE]
            .copy_from_slice(&self.version_valid_for_number.to_be_bytes());
        raw[VERSION_NUMBER_OFFSET..VERSION_NUMBER_OFFSET + VERSION_NUMBER_SIZE]
            .copy_from_slice(&self.version_number.to_be_bytes());
        raw
    }
}
