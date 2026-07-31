use crate::DbError;

pub trait SqliteFile {
    fn len(&self) -> Result<u64, DbError>;
    fn read_exact_at<B: AsMut<[u8]> + ?Sized>(
        &self,
        offset: u64,
        buff: &mut B,
    ) -> Result<(), DbError>;
    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buff: &B) -> Result<(), DbError>;
    fn set_len(&self, len: usize) -> Result<(), DbError>;
    fn sync(&self) -> Result<(), DbError>;
}
