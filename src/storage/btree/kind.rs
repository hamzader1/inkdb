use crate::SqliteResult;
use crate::errors::SqliteError;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::{IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use crate::storage::page::{BTreePage, LEFT_CHILD_POINTER_SIZE, OVERFLOW_POINTER_SIZE};
use crate::varint::encode_varint;
use crate::vfs::Vfs;

pub struct TableInterior;
pub struct TableLeaf;
pub struct IndexInterior;
pub struct IndexLeaf;

pub trait PageKind {
    type Cell: Cell;
    const BYTE: u8;
    const IS_LEAF: bool;
    const IS_INDEX: bool;
    const HEADER_SIZE: u8 = if Self::IS_LEAF { 8 } else { 12 };
}
impl PageKind for TableInterior {
    type Cell = TableInteriorCell;
    const BYTE: u8 = 0x05;
    const IS_LEAF: bool = false;
    const IS_INDEX: bool = false;

    const HEADER_SIZE: u8 = if Self::IS_LEAF { 8 } else { 12 };
}
impl PageKind for IndexInterior {
    type Cell = IndexInteriorCell;
    const BYTE: u8 = 0x02;
    const IS_LEAF: bool = false;
    const IS_INDEX: bool = true;
}
impl PageKind for TableLeaf {
    type Cell = TableLeafCell;
    const BYTE: u8 = 0x0d;
    const IS_LEAF: bool = true;
    const IS_INDEX: bool = false;
}
impl PageKind for IndexLeaf {
    type Cell = IndexLeafCell;
    const BYTE: u8 = 0x0a;
    const IS_LEAF: bool = true;
    const IS_INDEX: bool = true;
}
#[allow(clippy::len_without_is_empty)]
pub trait Cell: Sized {
    fn parse(bytes: &[u8], usable_size: usize) -> SqliteResult<Self>;
    fn len(&self) -> usize;
    fn rebase(&mut self, base: usize);
}
impl Cell for TableInteriorCell {
    fn parse(bytes: &[u8], usable_size: usize) -> SqliteResult<Self> {
        Self::parse(bytes, usable_size)
    }
    fn len(&self) -> usize {
        LEFT_CHILD_POINTER_SIZE + encode_varint(&mut [0u8; 9], self.rowid_boundary)
    }
    fn rebase(&mut self, _base: usize) {}
}
impl Cell for IndexInteriorCell {
    fn parse(bytes: &[u8], usable_size: usize) -> SqliteResult<Self> {
        Self::parse(bytes, usable_size)
    }
    fn len(&self) -> usize {
        let mut buffer = [0u8; 9];
        LEFT_CHILD_POINTER_SIZE
            + encode_varint(&mut buffer, self.payload_len)
            + (self.payload_range.end - self.payload_range.start)
            + {
                if self.first_overflow_page.is_some() {
                    OVERFLOW_POINTER_SIZE
                } else {
                    0
                }
            }
    }
    fn rebase(&mut self, base: usize) {
        self.payload_range.start += base;
        self.payload_range.end += base;
    }
}
impl Cell for TableLeafCell {
    fn parse(bytes: &[u8], usable_size: usize) -> SqliteResult<Self> {
        Self::parse(bytes, usable_size)
    }
    fn len(&self) -> usize {
        let mut buffer = [0u8; 9];
        encode_varint(&mut buffer, self.payload_len)
            + encode_varint(&mut buffer, self.row_id)
            + (self.payload_range.end - self.payload_range.start)
            + {
                if self.first_overflow_page.is_some() {
                    OVERFLOW_POINTER_SIZE
                } else {
                    0
                }
            }
    }
    fn rebase(&mut self, base: usize) {
        self.payload_range.start += base;
        self.payload_range.end += base;
    }
}
impl Cell for IndexLeafCell {
    fn parse(bytes: &[u8], usable_size: usize) -> SqliteResult<Self> {
        Self::parse(bytes, usable_size)
    }
    fn len(&self) -> usize {
        encode_varint(&mut [0u8; 9], self.payload_len)
            + (self.payload_range.end - self.payload_range.start)
            + {
                if self.first_overflow_page.is_some() {
                    OVERFLOW_POINTER_SIZE
                } else {
                    0
                }
            }
    }
    fn rebase(&mut self, base: usize) {
        self.payload_range.start += base;
        self.payload_range.end += base;
    }
}

pub trait HasRowId {
    fn row_id(&self) -> u64;
}
pub trait HasChild {
    fn left_child(&self) -> PageNo;
}
pub trait HasPayload {
    fn payload_range(&self) -> std::ops::Range<usize>;
    fn overflow_page(&self) -> Option<u32>;
    fn payload_len(&self) -> u64;
}

impl HasRowId for TableInteriorCell {
    fn row_id(&self) -> u64 {
        self.rowid_boundary
    }
}
impl HasRowId for TableLeafCell {
    fn row_id(&self) -> u64 {
        self.row_id
    }
}

impl HasChild for TableInteriorCell {
    fn left_child(&self) -> PageNo {
        self.left_child
    }
}

impl HasChild for IndexInteriorCell {
    fn left_child(&self) -> PageNo {
        self.left_child
    }
}
impl HasPayload for IndexInteriorCell {
    fn payload_range(&self) -> std::ops::Range<usize> {
        self.payload_range.clone()
    }
    fn overflow_page(&self) -> Option<u32> {
        self.first_overflow_page
    }
    fn payload_len(&self) -> u64 {
        self.payload_len
    }
}
impl HasPayload for IndexLeafCell {
    fn payload_range(&self) -> std::ops::Range<usize> {
        self.payload_range.clone()
    }
    fn overflow_page(&self) -> Option<u32> {
        self.first_overflow_page
    }
    fn payload_len(&self) -> u64 {
        self.payload_len
    }
}
impl HasPayload for TableLeafCell {
    fn payload_range(&self) -> std::ops::Range<usize> {
        self.payload_range.clone()
    }
    fn overflow_page(&self) -> Option<u32> {
        self.first_overflow_page
    }
    fn payload_len(&self) -> u64 {
        self.payload_len
    }
}
pub struct TypedPage<B, K: PageKind> {
    inner: BTreePage<B>,
    _kind: std::marker::PhantomData<fn() -> K>,
}

impl<B: AsRef<[u8]>, K: PageKind> TypedPage<B, K>
where
    K::Cell: HasChild,
{
    pub(crate) fn child(&self, i: u16) -> SqliteResult<u32> {
        Ok(self.cell(i)?.left_child())
    }
    pub(crate) fn rmp(&self) -> SqliteResult<u32> {
        self.inner
            .right_most_ptr()?
            .ok_or_else(|| SqliteError::Internal("interior page has no right-most pointer".into()))
    }
}
impl<B: AsRef<[u8]>, K: PageKind> TypedPage<B, K> {
    pub fn cell(&self, i: u16) -> SqliteResult<K::Cell> {
        let cell_offset = self.inner.cell_ptr(i)? as usize;
        let mut cell =
            K::Cell::parse(&self.inner.bytes()[cell_offset..], self.inner.usable_size())?;
        cell.rebase(cell_offset);
        Ok(cell)
    }
}

impl<B: AsRef<[u8]>, K: PageKind> TypedPage<B, K>
where
    K::Cell: HasRowId,
{
    pub(crate) fn row_id(&self, i: u16) -> SqliteResult<u64> {
        let cell_offset = self.inner.cell_ptr(i)? as usize;
        Ok(K::Cell::parse(&self.inner.bytes()[cell_offset..], self.inner.usable_size())?.row_id())
    }
    pub(crate) fn row_id_key(&self, i: u16) -> SqliteResult<Value<'static>> {
        Ok(Value::Integer(self.cell(i)?.row_id() as _))
    }
}

impl<B: AsRef<[u8]>, K: PageKind + IndexKind> TypedPage<B, K>
where
    K::Cell: HasPayload,
{
    pub(crate) fn index_payload_key<V: Vfs>(
        &self,
        i: u16,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>> {
        let cell = &self.cell(i)?;
        let record = self.inner.get_cell_record_v2(cell, pager)?;
        Ok(Value::Tuple(record).to_owned_static())
    }
}

impl<B, K: PageKind> TypedPage<B, K> {
    pub(crate) fn wrap(inner: BTreePage<B>) -> Self {
        Self {
            inner,
            _kind: std::marker::PhantomData,
        }
    }
}

pub enum AnyPage<B> {
    TableInterior(TypedPage<B, TableInterior>),
    TableLeaf(TypedPage<B, TableLeaf>),
    IndexInterior(TypedPage<B, IndexInterior>),
    IndexLeaf(TypedPage<B, IndexLeaf>),
}

impl<B: AsRef<[u8]>> AnyPage<B> {
    pub(crate) fn parse(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        b: B,
    ) -> SqliteResult<Self> {
        let btree_page = BTreePage::new(page_no, page_size, usable_size, b)?;
        match btree_page.page_type()?.as_byte() {
            TableInterior::BYTE => Ok(Self::TableInterior(TypedPage::wrap(btree_page))),
            TableLeaf::BYTE => Ok(Self::TableLeaf(TypedPage::wrap(btree_page))),
            IndexInterior::BYTE => Ok(Self::IndexInterior(TypedPage::wrap(btree_page))),
            IndexLeaf::BYTE => Ok(Self::IndexLeaf(TypedPage::wrap(btree_page))),
            other => Err(SqliteError::InvalidPageType(other)),
        }
    }
    pub(crate) fn cell_key<V: Vfs>(
        &self,
        i: u16,
        pager: &mut Pager<V>,
    ) -> SqliteResult<Value<'static>> {
        let key = {
            match self {
                AnyPage::TableInterior(p) => p.row_id_key(i)?,
                AnyPage::TableLeaf(p) => p.row_id_key(i)?,
                AnyPage::IndexInterior(p) => p.index_payload_key(i, pager)?,
                AnyPage::IndexLeaf(p) => p.index_payload_key(i, pager)?,
            }
        };
        Ok(key)
    }

    pub(crate) fn no_of_cells(&self) -> SqliteResult<u16> {
        match self {
            AnyPage::TableInterior(p) => p.no_of_cells(),
            AnyPage::TableLeaf(p) => p.no_of_cells(),
            AnyPage::IndexInterior(p) => p.no_of_cells(),
            AnyPage::IndexLeaf(p) => p.no_of_cells(),
        }
    }
}
impl<B: AsRef<[u8]>, K: PageKind> std::ops::Deref for TypedPage<B, K> {
    type Target = BTreePage<B>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub trait IndexKind: PageKind {}
impl IndexKind for IndexInterior {}
impl IndexKind for IndexLeaf {}
impl<B: AsRef<[u8]> + AsMut<[u8]>, K: PageKind> std::ops::DerefMut for TypedPage<B, K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
