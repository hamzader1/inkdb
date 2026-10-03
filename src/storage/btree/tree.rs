use crate::InkResult;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::kind::TypedPage;
use crate::storage::page::{BTreePage, PageMut, PageRef};
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

    pub fn insert(&mut self, key: &Value, content: &mut [u8]) -> InkResult<()> {
        self.insert_value(key, content)
    }

    pub fn delete(&mut self, key: Value) -> InkResult<bool> {
        self.delete_value(&key)
    }

    pub fn seek(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek(self.pager, target)
    }

    pub fn seek_lower_bound(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek_lower_bound(self.pager, target)
    }

    pub fn seek_for_delete(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek_for_delete(self.pager, target)
    }
    pub fn current_cell<K: PageKind>(&mut self) -> InkResult<Option<K::Cell>> {
        if let Some(path) = self.cursor.last_path() {
            let inner = BTreePage::new(
                path.page_no,
                self.pager.page_size(),
                self.pager.usable_size(),
                self.pager.header_len(),
                path.guard.bytes(),
            )?;
            if path.cell_idx >= inner.no_of_cells()? {
                return Ok(None);
            }
            return Ok(Some(
                TypedPage::<&[u8], K>::wrap(inner).cell(path.cell_idx)?,
            ));
        }
        Ok(None)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> InkResult<()> {
        self.cursor.next(self.pager)
    }

    pub fn prev(&mut self) -> InkResult<()> {
        self.cursor.prev(self.pager)
    }

    pub fn first(&mut self) -> InkResult<()> {
        self.cursor.first(self.pager)
    }

    pub fn last(&mut self) -> InkResult<()> {
        self.cursor.last(self.pager)
    }

    pub fn seek_into_first(&mut self) -> InkResult<()> {
        self.cursor.first(self.pager)
    }

    pub fn seek_into_last(&mut self) -> InkResult<()> {
        self.cursor.last(self.pager)
    }

    pub fn skip_past_end(&mut self) -> InkResult<()> {
        self.cursor.skip_past_end(self.pager)
    }

    pub fn save_position(&mut self) -> InkResult<()> {
        self.cursor.save_position(self.pager)
    }

    pub fn restore_position(&mut self) -> InkResult<RestorePosition> {
        self.cursor.restore_position(self.pager)
    }
    pub fn max_row_id(&mut self) -> InkResult<u64> {
        self.cursor.max_row_id(self.pager)
    }

    pub fn current_record<K: PageKind>(&mut self) -> InkResult<Option<Vec<Value<'_>>>>
    where
        K::Cell: HasPayload + Cell,
    {
        self.cursor.current_record::<K>(self.pager)
    }

    pub fn current_page_header_unchecked(&mut self) -> InkResult<u16> {
        let (page_no, _) = self.cursor.last_visited_entry_unchecked();
        self.with_page_ref(page_no, |page| page.no_of_cells())
    }

    pub fn allocate_page(&mut self) -> InkResult<PageNo> {
        self.pager.allocate_new_page()
    }

    pub fn deallocate_page(&mut self, page_no: PageNo) -> InkResult<()> {
        self.pager.dealloc(page_no)
    }

    pub fn with_page_ref<Func, R>(&mut self, page_no: PageNo, f: Func) -> InkResult<R>
    where
        Func: FnOnce(&PageRef<'_>) -> InkResult<R>,
    {
        let guard = self.pager.get(page_no)?;
        let page = page_as_ref_with_pager(page_no, &guard, self.pager)?;
        f(&page)
    }

    pub fn with_page_mut<Func, R>(&mut self, page_no: PageNo, f: Func) -> InkResult<R>
    where
        Func: FnOnce(&mut PageMut<'_>) -> InkResult<R>,
    {
        let mut guard = self.pager.get_mut(page_no)?;
        let mut page = super::page_as_mut_with_pager(page_no, &mut guard, self.pager)?;
        f(&mut page)
    }
}
