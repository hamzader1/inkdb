#![allow(unused)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;

use crate::bytes::*;
use crate::errors::SqliteError;
use crate::format::header;
use crate::seek_c;
use crate::seek_s;
use crate::sqlite_assert_all;
use crate::util::sqlite_assert_one;
use crate::vfs::cursor::FileCursor;
use crate::vfs::file::SqliteFile;

pub const SQLITE3_HEADER_SIZE: u8 = 100;

pub const SQLITE3_MAGIC: &[u8; HEADER_STRING_SIZE] = b"SQLite format 3\0";

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

pub const SQLITE_VERSION_NUMBER_OFFSET: usize = 96;
pub const SQLITE_VERSION_NUMBER_SIZE: usize = 4;
#[derive(Debug, Clone)]
pub struct SqliteDatabaseHeader {
    pub header_string: [u8; 16],
    pub database_page_size: u32,
    pub file_format_write_version: u8,
    pub file_format_read_version: u8,
    pub reserved_space: u8,
    pub maximum_embedded_payload_fraction: u8,
    pub minimum_embedded_payload_fraction: u8,
    pub leaf_payload_fraction: u8,
    pub file_change_counter: u32,
    pub database_size_in_pages: u32,
    pub first_freelist_trunk_page: u32,
    pub total_number_of_freelist_pages: u32,
    pub schema_cookie: u32,
    pub schema_format_number: u32,
    pub default_page_cache_size: u32,
    pub largest_root_btree_page: u32,
    pub database_text_encoding: u32,
    pub user_version: u32,
    pub incremental_vacuum_mode: u32,
    pub application_id: u32,
    pub reserved_for_expansion: [u8; 20],
    pub version_valid_for_number: u32,
    pub sqlite_version_number: u32,
}

impl SqliteDatabaseHeader {
    const INVALID_HEADER_ERR: SqliteError = SqliteError::InvalidDatabaseHeader;
    pub fn parse<R: SqliteFile>(sqlite_source: &'_ R) -> Result<Self, SqliteError> {
        // default cursor to 0, no manually offset needed
        let mut cursor = FileCursor::<'_, R>::new(sqlite_source);

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
        let sqlite_version_number = cursor.read_next_u32()?;
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
            sqlite_version_number,
        };
        header.validate()?;

        Ok(header)
    }

    fn validate(&mut self) -> Result<(), SqliteError> {
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
            sqlite_version_number,
        } = self;

        sqlite_assert_one(
            self.header_string == *SQLITE3_MAGIC,
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            self.database_page_size == 1
                || (self.database_page_size >= 512
                    && self.database_page_size <= 32768
                    && self.database_page_size.is_power_of_two()),
            SqliteError::InvalidPageSize(self.database_page_size as u16),
        )?;

        if self.database_page_size == 1 {
            self.database_page_size = 65536;
        }

        sqlite_assert_one(
            matches!(self.file_format_write_version, 1 | 2),
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            matches!(self.file_format_read_version, 1 | 2),
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            (self.reserved_space as u32) < self.database_page_size,
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            self.maximum_embedded_payload_fraction == 64,
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            self.minimum_embedded_payload_fraction == 32,
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(self.leaf_payload_fraction == 32, Self::INVALID_HEADER_ERR)?;

        sqlite_assert_one(
            matches!(self.schema_format_number, 1..=4),
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            matches!(self.database_text_encoding, 1..=3),
            Self::INVALID_HEADER_ERR,
        )?;

        sqlite_assert_one(
            self.reserved_for_expansion.iter().all(|&byte| byte == 0),
            Self::INVALID_HEADER_ERR,
        )?;
        Ok(())
    }
}
