use crate::DbError;

#[allow(clippy::len_without_is_empty)]
pub trait SqliteFile {
    fn len(&self) -> Result<u64, DbError>;

    fn read_exact_at(&self, offset: u64, buff: &mut [u8]) -> Result<(), DbError>;

    fn write_all_at(&self, offset: u64, buff: &[u8]) -> Result<(), DbError>;

    fn set_len(&self, len: usize) -> Result<(), DbError>;

    fn sync(&self) -> Result<(), DbError>;
}
