use crate::SqliteResult;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::page::{PageMut, PageRef};
use crate::vfs::Vfs;

use super::kind::{Cell, HasPayload, PageKind};
use super::{BTree, BTreeCursor, RestorePosition, SeekResult, page_as_ref_with_pager};

impl<'a, V: Vfs> BTree<'a, V> {
    pub fn with_cursor(pager: &'a mut Pager<V>, cursor: BTreeCursor<V>) -> Self {
        Self {
            root_page: cursor.root,
            pager,
            cursor,
        }
    }

    pub fn insert(&mut self, key: &Value, content: &mut [u8]) -> SqliteResult<()> {
        self.insert_value(key, content)
    }

    pub fn delete(&mut self, key: Value) -> SqliteResult<bool> {
        self.delete_value(&key)
    }

    pub fn seek(&mut self, target: &Value) -> SqliteResult<SeekResult> {
        self.cursor.seek(self.pager, target)
    }

    pub fn seek_lower_bound(&mut self, target: &Value) -> SqliteResult<SeekResult> {
        self.cursor.seek_lower_bound(self.pager, target)
    }

    pub fn seek_for_delete(&mut self, target: &Value) -> SqliteResult<SeekResult> {
        self.cursor.seek_for_delete(self.pager, target)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> SqliteResult<()> {
        self.cursor.next(self.pager)
    }

    pub fn prev(&mut self) -> SqliteResult<()> {
        self.cursor.prev(self.pager)
    }

    pub fn first(&mut self) -> SqliteResult<()> {
        self.cursor.first(self.pager)
    }

    pub fn last(&mut self) -> SqliteResult<()> {
        self.cursor.last(self.pager)
    }

    pub fn seek_into_first(&mut self) -> SqliteResult<()> {
        self.cursor.first(self.pager)
    }

    pub fn seek_into_last(&mut self) -> SqliteResult<()> {
        self.cursor.last(self.pager)
    }

    pub fn skip_past_end(&mut self) -> SqliteResult<()> {
        self.cursor.skip_past_end(self.pager)
    }

    pub fn save_position(&mut self) -> SqliteResult<()> {
        self.cursor.save_position(self.pager)
    }

    pub fn restore_position(&mut self) -> SqliteResult<RestorePosition> {
        self.cursor.restore_position(self.pager)
    }

    pub fn current_record<K: PageKind>(&mut self) -> SqliteResult<Option<Vec<Value<'_>>>>
    where
        K::Cell: HasPayload + Cell,
    {
        self.cursor.current_record::<K>(self.pager)
    }

    pub fn current_page_header_unchecked(&mut self) -> SqliteResult<u16> {
        let (page_no, _) = self.cursor.last_visited_entry_unchecked();
        self.with_page_ref(page_no, |page| page.no_of_cells())
    }

    pub fn allocate_page(&mut self) -> SqliteResult<PageNo> {
        self.pager.allocate_new_page()
    }

    pub fn deallocate_page(&mut self, page_no: PageNo) -> SqliteResult<()> {
        self.pager.dealloc(page_no)
    }

    pub fn with_page_ref<Func, R>(&mut self, page_no: PageNo, f: Func) -> SqliteResult<R>
    where
        Func: FnOnce(&PageRef<'_>) -> SqliteResult<R>,
    {
        let guard = self.pager.get(page_no)?;
        let page = page_as_ref_with_pager(page_no, &guard, self.pager)?;
        f(&page)
    }

    pub fn with_page_mut<Func, R>(&mut self, page_no: PageNo, f: Func) -> SqliteResult<R>
    where
        Func: FnOnce(&mut PageMut<'_>) -> SqliteResult<R>,
    {
        let mut guard = self.pager.get_mut(page_no)?;
        let mut page = super::page_as_mut_with_pager(page_no, &mut guard, self.pager)?;
        f(&mut page)
    }
}
