use crate::{
    InkResult,
    pager::pager::Pager,
    storage::btree::{BTreeCursor, RestorePosition},
    vfs::Vfs,
};

/// How a scan keeps its place while rows underneath it are changed.
///
/// A scan cannot hold the pages it is walking, so between one row and the next
/// it lets them go and remembers where it was. What it remembers, and what it
/// does when that place has moved, is the whole of this trait. The safe guard
/// relies on nothing changing the rows it is walking; the unsafe one follows the
/// row it was on, so a delete cannot make it skip or repeat.
pub trait ScanGuard<V: Vfs>: std::fmt::Debug {
    /// Put the cursor back where the scan was, stepping past the saved row when
    /// it is still there.
    fn restore(&mut self, _pager: &mut Pager<V>, _cursor: &mut BTreeCursor<V>) -> InkResult<()> {
        Ok(())
    }
    /// Remember where the scan is now, which is the current row or the one after
    /// it.
    fn save_or_advance(
        &mut self,
        _pager: &mut Pager<V>,
        _cursor: &mut BTreeCursor<V>,
    ) -> InkResult<()> {
        Ok(())
    }
    /// The guard name, for an EXPLAIN.
    fn scan_type(&self) -> &'static str;
}

/// Keeps the scan on the same row when a delete happens underneath it.
///
/// The position is saved as a key rather than a page and slot because the page
/// may be rebalanced out from under it. Restoring the position lands either on
/// the saved row, in which case the scan steps over it, or on its successor if
/// the row has been deleted. Either way, the cursor remains valid or the scan
/// is over.
#[derive(Debug)]
pub struct UnsafeScan;
impl<V: Vfs> ScanGuard<V> for UnsafeScan {
    fn restore(&mut self, pager: &mut Pager<V>, cursor: &mut BTreeCursor<V>) -> InkResult<()> {
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
    ) -> InkResult<()> {
        cursor.save_position(pager)
    }
    fn scan_type(&self) -> &'static str {
        "UnsafeScan"
    }
}

/// Assumes no row is changed underneath the scan, so it can simply step on.
///
/// This is what a read uses: nothing can be deleting rows while a plain SELECT
/// runs, so the cursor can walk page by page and never let go.
#[derive(Debug)]
pub struct SafeScan;
impl<V: Vfs> ScanGuard<V> for SafeScan {
    fn save_or_advance(
        &mut self,
        pager: &mut Pager<V>,
        cursor: &mut BTreeCursor<V>,
    ) -> InkResult<()> {
        cursor.next(pager)
    }
    fn scan_type(&self) -> &'static str {
        "SafeScan"
    }
}

/// Which guard a scan should use. The mode is decided when the plan is built,
/// since it depends on what the statement does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// For a statement that only reads.
    Safe,
    /// For a statement that deletes rows as it walks.
    Unsafe,
}

impl ScanMode {
    /// Build the guard this mode calls for.
    pub fn guard<V: Vfs>(self) -> Box<dyn ScanGuard<V>> {
        match self {
            Self::Safe => Box::new(SafeScan),
            Self::Unsafe => Box::new(UnsafeScan),
        }
    }
}
