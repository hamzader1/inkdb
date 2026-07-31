use crate::DbError;

pub trait SqliteFile {
    fn len(&self) -> Result<usize, DbError>;
    fn read_all_at<B: AsMut<[u8]> + ?Sized>(&self, offset: u64, buff: &mut B);
    fn write_all_at<B: AsRef<[u8]> + ?Sized>(&self, offset: u64, buff: &B);
    fn set_len(&mut self, len: usize) -> Result<(), DbError>;
    fn sync(&self) -> Result<(), DbError>;
}
