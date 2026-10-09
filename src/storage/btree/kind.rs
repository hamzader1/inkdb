use crate::InkResult;
use crate::errors::InkError;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::cell::{IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use crate::storage::page::{BTreePage, LEFT_CHILD_POINTER_SIZE, OVERFLOW_POINTER_SIZE};
use crate::varint::encode_varint;
use crate::vfs::Vfs;

/// A table interior page: row ids that divide the rows, and the children they
/// divide them between.
pub struct TableInterior;
/// A table leaf page: the rows themselves.
pub struct TableLeaf;
/// An index interior page: index entries that divide the keys, and the children
/// they divide them between.
pub struct IndexInterior;
/// An index leaf page: the index entries themselves.
pub struct IndexLeaf;

/// One of the four page kinds, as a type.
///
/// The constants let code that works over any kind ask what it is dealing with
/// without looking at the page.
pub trait PageKind {
    /// The cell this kind of page holds.
    type Cell: Cell;
    /// The page type byte.
    const BYTE: u8;
    /// Whether this page holds keys rather than children.
    const IS_LEAF: bool;
    /// Whether this page belongs to an index tree rather than a table.
    const IS_INDEX: bool;
    /// Bytes the page header takes, which is the leaf size or the interior size.
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
/// A cell of one of the four kinds.
///
/// The trait carries what the generic code has to do with a cell it does not
/// otherwise know the shape of.
#[allow(clippy::len_without_is_empty)]
pub trait Cell: Sized {
    fn parse(bytes: &[u8], usable_size: usize) -> InkResult<Self>;
    /// How many bytes the cell takes, which follows from its own contents.
    fn len(&self) -> usize;
    /// Shift the offsets inside the cell by the offset the cell was read at.
    ///
    /// Parsing works on the bytes from the cell onwards, so the payload range it
    /// finds is relative to that. This turns it back into a range in the page.
    fn rebase(&mut self, base: usize);
}
impl Cell for TableInteriorCell {
    fn parse(bytes: &[u8], usable_size: usize) -> InkResult<Self> {
        Self::parse(bytes, usable_size)
    }
    fn len(&self) -> usize {
        LEFT_CHILD_POINTER_SIZE + encode_varint(&mut [0u8; 9], self.rowid_boundary)
    }
    fn rebase(&mut self, _base: usize) {}
}
impl Cell for IndexInteriorCell {
    fn parse(bytes: &[u8], usable_size: usize) -> InkResult<Self> {
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
    fn parse(bytes: &[u8], usable_size: usize) -> InkResult<Self> {
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
    fn parse(bytes: &[u8], usable_size: usize) -> InkResult<Self> {
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

/// A cell that carries a row id.
pub trait HasRowId {
    /// The row id the cell sorts under.
    fn row_id(&self) -> u64;
}
/// A cell that carries a child page number.
pub trait HasChild {
    /// The page holding everything that sorts before this cell key.
    fn left_child(&self) -> PageNo;
}
/// A cell that carries a payload.
pub trait HasPayload {
    /// The part of the payload kept on the page.
    fn payload_range(&self) -> std::ops::Range<usize>;
    /// The first overflow page, when the payload had to spill.
    fn overflow_page(&self) -> Option<u32>;
    /// The full payload length, counting the part kept on overflow pages.
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
/// A page that carries its kind in its type.
///
/// Reading a cell from it gives back the cell type that belongs to that kind, so
/// the two can never drift apart.
pub struct TypedPage<B, K: PageKind> {
    inner: BTreePage<B>,
    _kind: std::marker::PhantomData<fn() -> K>,
}

impl<B: AsRef<[u8]>, K: PageKind> TypedPage<B, K>
where
    K::Cell: HasChild,
{
    pub(crate) fn _child(&self, i: u16) -> InkResult<u32> {
        Ok(self.cell(i)?.left_child())
    }
    /// The right-most child pointer, where the last subtree hangs off an interior page.
    pub(crate) fn rmp(&self) -> InkResult<u32> {
        self.inner.right_most_ptr()?.ok_or(InkError::Internal(
            "interior page has no right-most pointer",
        ))
    }
}
impl<B: AsRef<[u8]>, K: PageKind> TypedPage<B, K> {
    /// Read cell i as the cell type this page kind holds.
    pub fn cell(&self, i: u16) -> InkResult<K::Cell> {
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
    pub(crate) fn _row_id(&self, i: u16) -> InkResult<u64> {
        let cell_offset = self.inner.cell_ptr(i)? as usize;
        Ok(K::Cell::parse(&self.inner.bytes()[cell_offset..], self.inner.usable_size())?.row_id())
    }
    /// The key cell i sorts under, a row id as a value.
    pub(crate) fn row_id_key(&self, i: u16) -> InkResult<Value<'static>> {
        Ok(Value::Integer(self.cell(i)?.row_id() as _))
    }
}

impl<B: AsRef<[u8]>, K: PageKind + IndexKind> TypedPage<B, K>
where
    K::Cell: HasPayload,
{
    /// The key cell i sorts under: the whole entry as a tuple of values, since
    /// an index key is the record itself.
    pub(crate) fn index_payload_key<V: Vfs>(
        &self,
        i: u16,
        pager: &mut Pager<V>,
    ) -> InkResult<Value<'static>> {
        let cell = &self.cell(i)?;
        let record = self.inner.cell_record(cell, pager)?;
        Ok(Value::Tuple(record.into()).to_owned_static())
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

/// A page whose kind is worked out from its bytes rather than its type.
pub enum AnyPage<B> {
    /// A table interior page.
    TableInterior(TypedPage<B, TableInterior>),
    /// A table leaf page.
    TableLeaf(TypedPage<B, TableLeaf>),
    /// An index interior page.
    IndexInterior(TypedPage<B, IndexInterior>),
    /// An index leaf page.
    IndexLeaf(TypedPage<B, IndexLeaf>),
}

impl<B: AsRef<[u8]>> AnyPage<B> {
    pub fn parse(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        header_len: usize,
        b: B,
    ) -> InkResult<Self> {
        let btree_page = BTreePage::new(page_no, page_size, usable_size, header_len, b)?;
        match btree_page.page_type()?.as_byte() {
            TableInterior::BYTE => Ok(Self::TableInterior(TypedPage::wrap(btree_page))),
            TableLeaf::BYTE => Ok(Self::TableLeaf(TypedPage::wrap(btree_page))),
            IndexInterior::BYTE => Ok(Self::IndexInterior(TypedPage::wrap(btree_page))),
            IndexLeaf::BYTE => Ok(Self::IndexLeaf(TypedPage::wrap(btree_page))),
            other => Err(InkError::InvalidPageType(other)),
        }
    }
    /// The key cell i sorts under: a row id as an integer on a table page, and
    /// the whole entry as a tuple on an index page.
    pub fn cell_key<V: Vfs>(&self, i: u16, pager: &mut Pager<V>) -> InkResult<Value<'static>> {
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

    /// How many cells the page holds, whichever kind it turned out to be.
    pub(crate) fn no_of_cells(&self) -> InkResult<u16> {
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

/// A page kind that belongs to an index tree, where the keys are records.
pub trait IndexKind: PageKind {}
impl IndexKind for IndexInterior {}
impl IndexKind for IndexLeaf {}
impl<B: AsRef<[u8]> + AsMut<[u8]>, K: PageKind> std::ops::DerefMut for TypedPage<B, K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
