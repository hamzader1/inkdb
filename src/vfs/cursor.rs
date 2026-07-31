use super::file::SqliteFile;
use super::Vfs;

pub struct Cursor<'source, S> {
    s: &'source S,
    offset: u64,
}
impl<'source, S: SqliteFile> Cursor<'source, S> {
    fn new(s: &'source S) -> Self {
        Self { s, offset: 0 }
    }
    fn with_offset(s: &'source S, offset: u64) -> Self {
        let mut cur = Self::new(s);
        cur.offset = offset;
        cur
    }
    fn set_offset(&mut self, offset: u64) {
        self.offset = offset
    }
}
