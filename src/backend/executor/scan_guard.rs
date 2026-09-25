use crate::{
    SqliteResult,
    pager::pager::Pager,
    storage::btree::{BTreeCursor, RestorePosition},
    vfs::Vfs,
};

pub trait ScanGuard<V: Vfs>: std::fmt::Debug {
    fn restore(&mut self, pager: &mut Pager<V>, cursor: &mut BTreeCursor<V>) -> SqliteResult<()> {
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        Ok(())
    }
    fn scan_type(&self) -> &'static str;
}

// Unsafe means the row may vanish under us through Delete. When the
// saved row is still there we step over it, when it is gone the
// restore already parks on its successor. Either way the cursor is
// valid here or the scan is over.
#[derive(Debug)]
pub struct UnsafeScan;
impl<V: Vfs> ScanGuard<V> for UnsafeScan {
    fn restore(&mut self, pager: &mut Pager<V>, cursor: &mut BTreeCursor<V>) -> SqliteResult<()> {
        match cursor.restore_position(pager)? {
            RestorePosition::Exact => cursor.next(pager)?,
            RestorePosition::Next | RestorePosition::Empty => {}
        };
        Ok(())
    }
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        cursor.save_position(pager)
    }
    fn scan_type(&self) -> &'static str {
        "UnsafeScan"
    }
}

#[derive(Debug)]
pub struct SafeScan;
impl<V: Vfs> ScanGuard<V> for SafeScan {
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> SqliteResult<()> {
        cursor.next(pager)
    }
    fn scan_type(&self) -> &'static str {
        "SafeScan"
    }
}

pub struct CustomScanGuard<G, V>
where
    V: Vfs,
    G: FnOnce() -> Box<dyn ScanGuard<V>>,
{
    pub scan_guard: Option<G>,
}

impl<G, V> CustomScanGuard<G, V>
where
    V: Vfs,
    G: FnOnce() -> Box<dyn ScanGuard<V>>,
{
    pub fn new(scan_guard: Option<G>) -> Self {
        Self { scan_guard }
    }
    pub fn take(&mut self) -> G::Output {
        (self.scan_guard.take().unwrap())()
    }
}
