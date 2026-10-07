use super::btree::CellIndex;
use super::btree::kind::HasPayload;
use super::cell::{IndexInteriorCell, IndexLeafCell, TableInteriorCell, TableLeafCell};
use super::mem_cursor::MemCursor;
use crate::InkResult;
use crate::errors::{CorruptError, InkError};
use crate::pager::pager::PageNo;
use crate::pager::pager::Pager;
use crate::record::Value;
use crate::record::tuple::Tuple;
use crate::record::tuple::{decode_sqltype, into_borrowed, into_owned};
use crate::util::{
    assert_one, assert_with_corrupt_err, assert_with_internal_err, assert_with_runtime_err,
};
use crate::varint::encode_varint;
use crate::vfs::Vfs;

/// Bytes a leaf page spends on its header before the cell pointers start.
pub const LEAF_BTREE_PAGE_HEADER_SIZE: u8 = 8;
/// Bytes an interior page spends on its header, four more than a leaf for the
/// right-most child pointer.
pub const INTERIOR_BTREE_PAGE_HEADER_SIZE: u8 = 12;

/// Where the page type byte sits, relative to the start of the header.
pub const BTREE_TYPE_PAGE_OFFSET: u8 = 0;
/// Bytes the page type takes.
pub const BTREE_TYPE_PAGE_SIZE: u8 = 1;

/// Where the offset of the first freeblock sits, relative to the start of the
/// header.
pub const FIRST_FREEBLOCK_OFFSET: usize = 1;
/// Bytes that offset takes.
pub const FIRST_FREEBLOCK_SIZE: usize = 2;

/// Where the number of cells sits, relative to the start of the header.
pub const CELL_COUNT_OFFSET: usize = 3;
/// Bytes the cell count takes.
pub const CELL_COUNT_SIZE: usize = 2;

/// Where the start of the cell content area sits, relative to the start of the
/// header.
pub const CELL_CONTENT_AREA_OFFSET: usize = 5;
/// Bytes that offset takes.
pub const CELL_CONTENT_AREA_SIZE: usize = 2;

/// Where the count of fragmented free bytes sits, relative to the start of the
/// header.
pub const FRAGMENTED_FREE_BYTES_OFFSET: usize = 7;
/// Bytes that count takes, one.
pub const FRAGMENTED_FREE_BYTES_SIZE: usize = 1;

/// Where the right-most child pointer sits, relative to the start of the header.
pub const RIGHT_MOST_POINTER_OFFSET: usize = 8;
/// Bytes the right-most child pointer takes.
pub const RIGHT_MOST_POINTER_SIZE: usize = 4;

/// Bytes a cell spends on the page number of its left child.
pub const LEFT_CHILD_POINTER_SIZE: usize = 4;
/// Bytes a cell spends on the page number of its first overflow page.
pub const OVERFLOW_POINTER_SIZE: usize = 4;

/// Bytes the database header takes at the start of page one. Every other page
/// has no such header, so its b-tree page begins at byte zero.
pub const HEADER_SIZE: usize = 100;

/// Which of the four shapes a b-tree page takes.
///
/// The number each variant carries is the byte written at the start of such a
/// page, the same one SQLite uses.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum BTreePageType {
    /// An interior page of an index tree.
    InteriorIndex = 0x02,
    /// A leaf page of an index tree.
    LeafIndex = 0x0a,
    /// An interior page of a table tree.
    InteriorTable = 0x05,
    /// A leaf page of a table tree.
    LeafTable = 0x0d,
}

impl BTreePageType {
    fn get(kind: u8) -> Option<Self> {
        match kind {
            0x0a => Some(Self::LeafIndex),
            0x02 => Some(Self::InteriorIndex),
            0x0d => Some(Self::LeafTable),
            0x05 => Some(Self::InteriorTable),
            _ => None,
        }
    }

    /// Whether this page holds keys rather than children.
    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::LeafIndex | Self::LeafTable)
    }

    /// Whether this page holds children.
    pub fn is_interior(&self) -> bool {
        matches!(self, Self::InteriorTable | Self::InteriorIndex)
    }
    /// The byte this type is written as.
    pub fn as_byte(&self) -> u8 {
        match self {
            Self::LeafIndex => 0x0a,
            Self::InteriorIndex => 0x02,
            Self::LeafTable => 0x0d,
            Self::InteriorTable => 0x05,
        }
    }
    /// How many bytes a page of this type spends on its header.
    pub fn header_size(&self) -> u8 {
        match self {
            Self::InteriorIndex | Self::InteriorTable => INTERIOR_BTREE_PAGE_HEADER_SIZE,
            _ => LEAF_BTREE_PAGE_HEADER_SIZE,
        }
    }
}

impl TryFrom<u8> for BTreePageType {
    type Error = InkError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::get(value).ok_or(InkError::InvalidPageType(value))
    }
}

/// What an insert did with a cell.
#[derive(Debug, PartialEq)]
pub enum InsertionState {
    /// The cell went in.
    Inserted,
    /// The cell did not fit, so the page was left as it was.
    None,
}

/// A page in an overflow chain, as its bytes present it.
///
/// An overflow page opens with the number of the next page in the chain, zero
/// when it is the last, and then carries a slice of the payload that would not
/// fit on the b-tree page itself.
pub(crate) struct OverflowPageRef<'a> {
    /// The next page in the chain, or zero at the end.
    pub next: PageNo,
    /// The payload this page carries.
    pub data: &'a [u8],
}

impl<'a> OverflowPageRef<'a> {
    pub fn new<T: AsRef<[u8]> + ?Sized>(
        bytes: &'a T,
        usable_size: usize,
    ) -> Result<Self, InkError> {
        let data = bytes.as_ref();
        assert_with_corrupt_err(data.len() >= usable_size, || {
            "not enough bytes in overflow page".into()
        })?;

        let next_page_buffer = match data[0..4].as_array::<4>() {
            Some(buf) => buf,
            _ => {
                return Err(InkError::Corrupt(CorruptError::OverflowNextPointer));
            }
        };
        let next_page = u32::from_be_bytes(*next_page_buffer);
        let data = &data[4..(usable_size)];

        Ok(Self {
            next: next_page,
            data,
        })
    }
}

/// A freeblock: a run of free bytes inside a page's cell content area.
///
/// It opens with the offset of the next freeblock, zero at the end of the list,
/// and then its own size. It is never shorter than four bytes, since anything
/// smaller would leave no room for those two numbers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FreeCell {
    /// Where the freeblock begins in the page.
    pub starting_offset: u16,
    /// The offset of the next freeblock, zero at the end of the list.
    pub next: u16,
    /// How many bytes the freeblock covers.
    pub size: u16,
}

impl FreeCell {
    pub fn parse(ptr: u16, bytes: &[u8]) -> InkResult<Self> {
        let mut cursor = MemCursor::with_offset(bytes, ptr as _)?;
        let next = cursor.read_next_u16()?;
        let size = cursor.read_next_u16()?;
        Ok(Self {
            starting_offset: ptr,
            next,
            size,
        })
    }
}

impl<'a> OverflowPageRef<'a> {
    /// Put a split payload back together.
    ///
    /// `local_payload_bytes` is the part that stayed on the b-tree page; the
    /// rest is read from the chain that starts at `first_overflow_page`. The
    /// chain has to end exactly when the payload does, so a chain that runs on
    /// or stops short is treated as a corrupt page.
    ///
    /// # Overflow page visualisatio
    /// Example of a [`IndexLeafCell`] with one overflow page.
    ///
    /// ```text
    ///
    ///      varint       byte array      4 bytes
    /// +-------------+----------------+---------------+
    /// | Payload len |  Payload....   | Overflow page +---+
    /// +-------------+----------------+---------------+   |
    ///                  Local payload                     |
    ///                                                    |    +---------------+
    ///                                                    +--->|     Next      +---->  NULL
    ///                                                         | overflowpage  |
    ///                                                         +---------------+
    ///                                                         |               |
    ///                                                         |               |
    ///                                                         |               |
    ///                                                         |     data      |
    ///                                                         |               |
    ///                                                         |               |
    ///                                                         |               |
    ///                                                         |               |
    ///                                                         +---------------+
    ///
    /// ```
    /// # Errors
    /// When the declared length is shorter than the bytes already held, when the
    /// chain does not line up with the payload, or when the total falls short of
    /// what was promised.
    pub fn get_total_payload<V: crate::vfs::Vfs>(
        pager: &mut Pager<V>,
        local_payload_bytes: &[u8],
        total_payload_length: usize,
        usable_size: usize,
        first_overflow_page: PageNo,
    ) -> Result<Vec<u8>, InkError> {
        let mut remaining = total_payload_length
            .checked_sub(local_payload_bytes.len())
            .ok_or(InkError::Corrupt(CorruptError::OverflowPayloadTooLong))?;
        let mut current_page = first_overflow_page;
        let mut total_collected_payload: Vec<u8> = Vec::new();
        total_collected_payload.extend_from_slice(local_payload_bytes);
        while remaining > 0 {
            let page = pager.get(current_page)?;
            let buffer = page.bytes();
            let overflow_page = OverflowPageRef::new(&buffer, usable_size as _)?;
            let bytes_to_read: usize = remaining.min(overflow_page.data.len());
            total_collected_payload.extend_from_slice(&overflow_page.data[..bytes_to_read]);
            remaining -= bytes_to_read;
            if remaining == 0 {
                if overflow_page.next != 0 {
                    return Err(InkError::CorruptedPage {
                        page: current_page,
                        reason: "overflow chain continues after payload is complete".into(),
                    });
                }
                break;
            }
            if overflow_page.next == 0 {
                return Err(InkError::CorruptedPage {
                    page: current_page,
                    reason: "overflow chain ends before payload is complete".into(),
                });
            }
            current_page = overflow_page.next;
        }

        assert_one(
            total_collected_payload.len() == total_payload_length,
            InkError::Corrupt(CorruptError::OverflowPayloadMismatch),
        )?;

        Ok(total_collected_payload)
    }
}

/// How many bytes a cell spends pointing at its first overflow page: four when
/// there is one, none when there is not.
fn overflow_pointer_len(first_overflow_page: Option<PageNo>) -> usize {
    if first_overflow_page.is_some() {
        OVERFLOW_POINTER_SIZE
    } else {
        0
    }
}

/*
   ** X is U-35 for table btree leaf pages or ((U-12)*64/255)-23 for index pages.
   ** M is always ((U-12)*32/255)-23.
   ** Let K be M+((P-M)%(U-4)).
   ** If P<=X then all P bytes of payload are stored directly
       on the btree page without overflow.

   ** If P>X and K<=X then the first K bytes of P are stored
       on the btree page and the remaining P-K bytes are stored
       on overflow pages.

   ** If P>X and K>X then the first M bytes of P are stored on
       the btree page and the remaining P-M bytes are stored on
       overflow pages.
*/
/// How much of a table cell's payload stays on the b-tree page.
///
/// SQLite only spills payload once it passes X, and then keeps either M or K
/// bytes, chosen so the page never ends up with a sliver of payload stuck on it
/// that is too small to be worth carrying. Anything above that goes to an
/// overflow chain.
pub fn compute_table_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = u - 35;
    if p <= x {
        p
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((p - m) % (u - 4));
        if k <= x { k } else { m }
    }
}
/// How much of an index cell's payload stays on the b-tree page.
///
/// The same rule as the table version, but with X worked out for index pages,
/// which hold less because every index cell also carries a child pointer.
pub fn compute_index_local_payload_size(usable_size: usize, payload_len: usize) -> usize {
    let u = usable_size;
    let p = payload_len;
    let x = ((u - 12) * 64 / 255) - 23;
    if p <= x {
        p
    } else {
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((p - m) % (u - 4));
        if k <= x { k } else { m }
    }
}

/// A page seen through bytes that are borrowed for reading.
pub type PageRef<'a> = BTreePage<&'a [u8]>;
/// A page seen through bytes that are borrowed for writing.
pub(crate) type PageMut<'a> = BTreePage<&'a mut [u8]>;

/// A b-tree page.
///
/// # Structure of a tree page
/// ```text
///
/// ┌────────────────┐
/// │  Page header   │ 12 bytes for internal node page
/// │                │ 8 bytes for leaf node page
/// ├────────────────┤
/// │  Cell pointer  ││
/// │     array      ││ 2 bytes per cell. Sorted order
/// ├────────────────┤▼
/// │  Unallocated   │
/// │     space      │
/// │                │
/// ├────────────────┤
/// │                │▲
/// │                ││ Arbitrary order
/// │  Cell content  ││ interspersed
/// │      area      ││ with free space
/// │                ││
/// │                ││
/// └────────────────┘
/// ```
/// We note that the first page (Page 1) has a 100 byte file
/// header that resides before the page header.
/// The cell pointer array and the cell content area grow toward each other (via the middle unallocated space)
/// just like two stacks are placed facing one another.
/// The cell pointer array acts as the pagedirectory that helps in mapping
/// logical cell order to their physical cell storage in the cell content area.
///
/// # The page header
/// The structure of page header is given below:
///  ``` text
///
/// +--------+------+--------------------------------------------------------------+
/// | Offset | Size | Description                                                  |
/// +--------+------+--------------------------------------------------------------+
/// | 0      | 1    | Flags. 2: internal index-tree page                           |
/// |        |      |        5: internal table-tree page                           |
/// |        |      |       10: leaf index-tree page                               |
/// |        |      |       13: leaf table-tree page                               |
/// +--------+------+--------------------------------------------------------------+
/// | 1      | 2    | Byte offset to the first free block                          |
/// +--------+------+--------------------------------------------------------------+
/// | 3      | 2    | Number of cells on this page                                 |
/// +--------+------+--------------------------------------------------------------+
/// | 5      | 2    | Offset to the first byte of the cell content area            |
/// +--------+------+--------------------------------------------------------------+
/// | 7      | 1    | Number of fragmented free bytes                              |
/// +--------+------+--------------------------------------------------------------+
/// | 8      | 4    | Right child (the Ptr(n) value). Omitted on leaves            |
/// +--------+------+--------------------------------------------------------------+
/// ```
///
/// # Structure of storage area
/// Cells are stored at the very end of the page (high address), and they grow toward the beginning of the page.
/// The cell pointer array begins on the first byte after the page header, and it contains zero or more cell pointers.
/// The number of elements in the array is stored in the page header at offset 3.
/// Each cell pointer is a 2-byte integer number indicating an offset (from the beginning of the page)
/// to the actual cell within the cell content area. The cell pointers are stored in sorted
/// order (by the corresponding key values), even though cells may be stored unordered.
/// The left entries have smaller key values than the right entries. Cells are not necessarily contiguous or in order.
/// ``` text
///                ┌───────────────┐
///                │  Page header  │
///   Cell         ├──┬──┬──┬──────┤   ▲
/// pointer ──────▶│  │  │  │──────┼─┐ │
///  array         ├──┴──┴──┘      │ │ │
///                │ │  │          │ │ │
///                │ └──┼──┐       │ │ │
///           ┌────┼────┘  ▼       │ │ │
///           │    │┌───────┐      │ │ │
///           │    ││Cell 1 │      │ │ │  Usable
///           │    │└───────┘      │ │ │  Space
///           │    │┌───────────┐  │ │ │
///           │    ││  Cell 3   │ ◀┼─┘ │
///           │    │└───────────┘  │   │
///           │    │┌─────────┐    │   │
///           └───▶││ Cell 2  │    │   │
///                │└─────────┘    │   │
///                ├ ─ ─ ─ ─ ─ ─ ─ ┤   ▼
///       Reserved │               │
///                └───────────────┘
/// ```
///
/// Our [`BTreePage`] wraps the page's bytes and knows how to read and change the header, the
/// cell pointer array and the cells themselves. The byte type is generic, so one
/// piece of code serves a shared view, an exclusive one and an owned buffer:
/// reading is all most of it needs, and only the methods that change the page ask
/// for a mutable view.
#[derive(Debug)]
pub struct BTreePage<B> {
    page_no: PageNo,
    header_offset: u8,
    page_size: usize,
    usable_size: usize,
    bytes: B,
}

impl<B: AsRef<[u8]>> BTreePage<B> {
    /// Check the page against the rules a b-tree page has to keep: the pointer
    /// array fits before the content area, every pointer lands inside the
    /// content area, no two cells overlap, and an interior page that holds cells
    /// has a right-most pointer. Panics when one of them is broken.
    pub fn assert_invariants(&self) -> InkResult<()> {
        let hdr = self.header_size()? as usize;
        let n = self.no_of_cells()? as usize;
        let cca = self.cell_content_area()? as usize;
        assert!(hdr + 2 * n <= cca, "pointer array overruns content area");
        assert!(cca <= self.usable_size, "content area past usable size");
        let mut spans: Vec<std::ops::Range<usize>> = Vec::with_capacity(n);
        for i in 0..n {
            let p = self.cell_ptr(i as u16)? as usize;
            assert!(
                p >= hdr && p < self.usable_size,
                "cell {} pointer {} outside content area",
                i,
                p
            );
            spans.push(self.cell_span(p as u16)?);
        }
        spans.sort_by_key(|s| s.start);
        for w in spans.windows(2) {
            assert!(
                w[0].end <= w[1].start,
                "cells overlap: {:?} / {:?}",
                w[0],
                w[1]
            );
        }
        if self.page_type()?.is_interior() && n > 0 {
            assert!(
                self.right_most_ptr()?.is_some(),
                "populated interior page without right-most pointer"
            );
        }
        Ok(())
    }
    /// The number this page was read from.
    pub fn page_no(&self) -> PageNo {
        self.page_no
    }
}

impl<B: AsRef<[u8]>> BTreePage<B> {
    pub fn new(
        page_no: PageNo,
        page_size: usize,
        usable_size: usize,
        header_len: usize,
        bytes: B,
    ) -> InkResult<Self> {
        let header_offset = if page_no == 1 { header_len as u8 } else { 0 };
        let this = BTreePage {
            page_no,
            page_size,
            usable_size,
            header_offset,
            bytes,
        };
        BTreePageType::try_from(this.u8_at(header_offset as _)?)?;
        Ok(this)
    }
    /// How many bytes of the page are usable, with the reserved space at the end left out.
    pub fn usable_size(&self) -> usize {
        self.usable_size
    }
    /// How many bytes the page takes in the file.
    pub fn page_size(&self) -> usize {
        self.page_size
    }
    /// The page's bytes.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }
    /// Read one byte at an offset.
    pub fn u8_at(&self, off: usize) -> InkResult<u8> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u8())
    }
    /// Read two bytes at an offset as a big endian number.
    pub fn u16_at(&self, off: usize) -> InkResult<u16> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u16())
    }
    /// Read four bytes at an offset as a big endian number.
    pub fn u32_at(&self, off: usize) -> InkResult<u32> {
        self.with_cursor_read_at(off, |cursor| cursor.read_next_u32())
    }
    /// Put a cursor at an offset and read through it. The small readers above
    /// all go through here.
    pub fn with_cursor_read_at<F, R>(&self, off: usize, f: F) -> InkResult<R>
    where
        F: FnOnce(&mut MemCursor) -> InkResult<R>,
    {
        let mut cursor = MemCursor::with_offset(self.bytes(), off as _)?;
        f(&mut cursor)
    }
    /// Turn an offset within the header into an offset within the page. Only
    /// page one shifts anything, since its b-tree header comes after the
    /// database header.
    pub fn with_header_offset(&self, off: usize) -> usize {
        self.header_offset as usize + off
    }
    /// How many bytes this page's header takes, counting the offset it starts at.
    pub fn header_size(&self) -> InkResult<u8> {
        Ok(self.header_offset + self.page_type()?.header_size())
    }
    /// Which of the four shapes this page is, read from the first header byte.
    pub fn page_type(&self) -> InkResult<BTreePageType> {
        BTreePageType::try_from(self.u8_at(self.header_offset as _)?)
    }
    /// How many cells the page holds.
    pub fn no_of_cells(&self) -> InkResult<u16> {
        self.u16_at(self.with_header_offset(CELL_COUNT_OFFSET))
    }
    /// Where the cell content area starts. Cells are packed from the end of the
    /// page working down, so this number falls as the page fills up.
    pub fn cell_content_area(&self) -> InkResult<u16> {
        self.u16_at(self.with_header_offset(CELL_CONTENT_AREA_OFFSET))
    }
    /// How many fragmented free bytes the page has, gaps too small to be freeblocks.
    ///
    /// A freeblock must be at least 4 bytes. Smaller blocks are treated as fragments.
    fn frag_cnt(&self) -> InkResult<u8> {
        self.u8_at(self.with_header_offset(FRAGMENTED_FREE_BYTES_OFFSET))
    }
    /// The right-most child pointer, which only an interior page has. A leaf
    /// page answers with nothing.
    pub fn right_most_ptr(&self) -> InkResult<Option<u32>> {
        if self.page_type()?.is_interior() {
            return Ok(Some(
                self.u32_at(self.with_header_offset(RIGHT_MOST_POINTER_OFFSET))?,
            ));
        }
        Ok(None)
    }
    /// The offset of the first freeblock, or zero when the page has none.
    ///
    /// Because of random inserts and deletes of cells on a page, the page may have cells
    /// and free space interspersed (inside the cell content area).
    /// The unused space within the cell content area is collected into a singly linked list of free blocks.
    pub fn first_freeblock(&self) -> InkResult<u16> {
        self.u16_at(self.with_header_offset(FIRST_FREEBLOCK_OFFSET))
    }

    /// The offset of cell i, read from the cell pointer array.
    ///
    /// # Errors
    /// When i is past the last cell.
    pub fn cell_ptr(&self, i: u16) -> InkResult<u16> {
        if i >= self.no_of_cells()? {
            return Err(InkError::InvalidCellPointer(i));
        }
        self.u16_at((self.header_size()? as u16 + i * 2) as usize)
    }
    #[allow(clippy::chunks_exact_to_as_chunks)]
    /// The offset of every cell, in order.
    /// The cell pointer array begins on the first byte after the page header,
    /// and it contains zero or more cell pointers.
    pub fn cell_ptrs(&self) -> InkResult<impl Iterator<Item = u16> + '_> {
        let start = self.header_size()? as usize;
        let no_of_cells = self.no_of_cells()? as usize;
        Ok(self.bytes()[start..start + no_of_cells * 2]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]])))
    }

    /// How many free bytes the page has, adding up the freeblocks, the
    /// fragmented bytes and the gap above the content area.
    pub fn freespace(&self) -> InkResult<usize> {
        let freeblocks_size = self.total_freeblocks_size()?;
        let total_free_bytes =
            freeblocks_size + self.frag_cnt()? as usize + self.cell_content_area()? as usize
                - (self.header_size()? as u16 + self.no_of_cells()? * 2) as usize;
        Ok(total_free_bytes)
    }
    /// Walk the freeblock list and add up the sizes.
    fn total_freeblocks_size(&self) -> InkResult<usize> {
        if self.first_freeblock()? == 0 {
            return Ok(0);
        }
        let mut cursor = MemCursor::with_offset(self.bytes(), self.first_freeblock()? as u64)?;
        let mut total_size = 0;
        let mut next_freeblock_offset = cursor.read_next_u16()?;
        let mut freeblock_size = cursor.read_next_u16()?;
        total_size += freeblock_size;
        while next_freeblock_offset != 0 {
            cursor.set_offset(next_freeblock_offset as _);
            next_freeblock_offset = cursor.read_next_u16()?;
            freeblock_size = cursor.read_next_u16()?;
            total_size += freeblock_size;
        }

        Ok(total_size as _)
    }
    /// Whether the page has emptied out past the point where it is worth
    /// keeping, more than two thirds free.
    pub fn is_underflow(&self) -> InkResult<bool> {
        Ok(self.freespace()? > self.usable_size * 2 / 3)
    }
    /// Read the values of a cell.
    /// # Deserialize Algorithm
    /// Both [`BTreePage::decode_loop_owned`] and [`BTreePage::decode_loop_borrowed`]
    /// follow the same approach.
    ///
    /// We initialize a [`MemCursor`] at the start of the payload and read the first
    /// varint, which gives us the header length.
    ///
    /// Let's assume we have this payload:
    ///
    /// ```text
    ///                                          1     A      l       i     c      e     22
    ///     +------------+------+------+------+------+-----+------+------+------+------+-----+
    ///     |     04     |  01  |  17  |  01  |  01  | 41  |  6c  |  69  |  63  |  65  | 16  |
    ///     +------------+------+------+------+------+-----+------+------+------+------+-----+
    ///     |            |                    |                                              |
    ///     +-Header-len-+-------Data types---+---------------Payload------------------------+
    ///
    /// ```
    ///
    /// The payload above is a serialized record of the row `[id=1, name="Alice", age=22]`.
    /// The first varint is `4`, which is the header length. Since the varint itself
    /// takes one byte, there are 3 bytes left in the header.
    ///
    /// The header pointer is now positioned after the header length. We initialize
    /// a second pointer, `data_pointer`, at the same position and move it forward by
    /// 3 bytes so that it points to the start of the payload.
    ///
    /// ```text
    ///
    ///           Header pointer         data pointer
    ///                  |                    |
    ///                  v                    v
    ///     +------------+------+------+------+------+-----+------+------+------+------+-----+
    ///     |     04     |  01  |  17  |  01  |  01  | 41  |  6c  |  69  |  63  |  65  | 16  |
    ///     +------------+------+------+------+------+-----+------+------+------+------+-----+
    ///     |            |                    |                                              |
    ///     +-Header-len-+-------Data types---+---------------Payload------------------------+
    ///
    /// ```
    ///
    /// We read the next varint and get `1`, which tells us that the value takes one
    /// byte. We read that byte using the data pointer, then continue the same
    /// process until all values have been decoded.
    /// See [`BTreePage::decode_loop_owned`] and [`BTreePage::decode_loop_borrowed`]
    ///
    /// When the payload spilled onto overflow pages it is gathered first, and
    /// the values then own their bytes, since they no longer point into this
    /// page.
    pub fn cell_record<V: Vfs, C>(
        &self,
        cell: &C,
        pager: &mut Pager<V>,
    ) -> InkResult<Vec<Value<'_>>>
    where
        C: HasPayload,
    {
        let mut collector = Vec::new();
        let cell_payload = cell.payload_range();
        let ovp = cell.overflow_page();
        if let Some(overflow_page) = ovp {
            let vec = OverflowPageRef::get_total_payload(
                pager,
                &self.bytes()[cell_payload],
                cell.payload_len() as _,
                self.usable_size,
                overflow_page,
            )?;
            self.decode_loop_owned(vec, &mut collector)?;
        } else {
            self.decode_loop_borrowed(&self.bytes()[cell.payload_range()], &mut collector)?;
        }
        Ok(collector)
    }

    /// The raw record bytes of a cell, put back together from this page and any
    /// overflow pages it spilled onto.
    pub fn cell_record_as_bytes<V: Vfs, C>(
        &self,
        cell: &C,
        pager: &mut Pager<V>,
    ) -> InkResult<Vec<u8>>
    where
        C: HasPayload,
    {
        match cell.overflow_page() {
            Some(overflow_page) => OverflowPageRef::get_total_payload(
                pager,
                &self.bytes()[cell.payload_range()],
                cell.payload_len() as usize,
                self.usable_size,
                overflow_page,
            ),
            None => Ok(self.bytes()[cell.payload_range()].to_vec()),
        }
    }

    /// Decode a record held in its own buffer, pushing each value into the collector.
    fn decode_loop_owned(
        &self,
        bytes: Vec<u8>,
        collector: &mut Vec<Value<'_>>,
    ) -> Result<(), InkError> {
        let mut header_cursor = MemCursor::new(bytes.as_slice()); /*P1*/
        let (header_size, consumed) = header_cursor.read_next_varint(bytes.len())?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: MemCursor = header_cursor.clone_with_offset(header_size)?; /*P2*/
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(bytes.len())?;
            let record_metadata = Tuple::content_meta(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            let decoded = decode_sqltype(data, &record_metadata);
            collector.push(into_owned(decoded));
            remaining -= consumed;
        }
        Ok(())
    }

    /// The same walk over a record that lives in borrowed bytes, where the values point into it.
    fn decode_loop_borrowed<'a>(
        &self,
        bytes: &'a [u8],
        collector: &mut Vec<Value<'a>>,
    ) -> Result<(), InkError> {
        let mut header_cursor = MemCursor::new(bytes); /*P1*/
        let (header_size, consumed) = header_cursor.read_next_varint(bytes.len())?;
        let mut remaining = (header_size as usize) - consumed;
        let mut data_cursor: MemCursor = header_cursor.clone_with_offset(header_size)?; /*P2*/
        while remaining > 0 {
            let (serial_type, consumed) = header_cursor.read_next_varint(bytes.len())?;
            let record_metadata = Tuple::content_meta(serial_type);
            let data = data_cursor.read_to(record_metadata.size as _)?;
            let decoded = decode_sqltype(data, &record_metadata);
            collector.push(into_borrowed(decoded));
            remaining -= consumed;
        }
        Ok(())
    }
    /// Whether this page belongs to an index tree rather than a table.
    pub fn is_index(&self) -> InkResult<bool> {
        let page_type = self.page_type()?;
        Ok(matches!(
            page_type,
            BTreePageType::LeafIndex | BTreePageType::InteriorIndex
        ))
    }

    /// The byte range cell i takes.
    ///
    /// The end is worked out from the cell's own contents, which are laid out
    /// differently for each page type, so this reads the cell rather than
    /// assuming a size.
    pub fn cell_span(&self, cell_ptr: u16) -> InkResult<std::ops::Range<usize>> {
        let start = cell_ptr as usize;
        assert_with_corrupt_err(
            start >= self.header_size()? as usize && start < self.usable_size,
            || format!("cell pointer {start} outside content area"),
        )?;
        let bytes = &self.bytes()[start..self.usable_size];
        let end = match self.page_type()? {
            BTreePageType::LeafTable => {
                let cell = TableLeafCell::parse(bytes, self.usable_size)?;
                start + cell.payload_range.end + overflow_pointer_len(cell.first_overflow_page)
            }
            BTreePageType::LeafIndex => {
                let cell = IndexLeafCell::parse(bytes, self.usable_size)?;
                start + cell.payload_range.end + overflow_pointer_len(cell.first_overflow_page)
            }
            BTreePageType::InteriorTable => {
                let cell = TableInteriorCell::parse(bytes, self.usable_size)?;
                start + LEFT_CHILD_POINTER_SIZE + encode_varint(&mut [0u8; 9], cell.rowid_boundary)
            }
            BTreePageType::InteriorIndex => {
                let cell = IndexInteriorCell::parse(bytes, self.usable_size)?;
                start + cell.payload_range.end + overflow_pointer_len(cell.first_overflow_page)
            }
        };
        Ok(start..end)
    }
    /// The raw bytes of cell i.
    pub fn cell_bytes_as_ref(&self, cell_index: u16) -> InkResult<&[u8]> {
        let cell_offset = self.cell_ptr(cell_index)?;
        let cell_span = self.cell_span(cell_offset)?;
        Ok(&self.bytes()[cell_span])
    }
    /// How many bytes lie between the end of the pointer array and the start of
    /// the content area. A cell and its pointer fit in here without anything
    /// having to move.
    pub fn remaining_space(&self) -> InkResult<usize> {
        Ok(self.cell_content_area()? as usize
            - (self.no_of_cells()? * 2) as usize
            - (self.header_size()?) as usize)
    }

    /// Whether this page holds keys rather than children.
    pub fn is_leaf(&self) -> InkResult<bool> {
        Ok(self.page_type()?.is_leaf())
    }

    /// Whether this page holds children.
    pub fn is_interior(&self) -> InkResult<bool> {
        Ok(self.page_type()?.is_interior())
    }
}

impl<B: AsRef<[u8]> + AsMut<[u8]>> BTreePage<B> {
    pub fn new_from_raw_bytes(
        page_no: PageNo,
        page_kind: BTreePageType,
        mut bytes: B,
        page_size: usize,
        usable_size: usize,
        header_len: usize,
    ) -> InkResult<Self> {
        let header_offset = if page_no == 1 { header_len as u8 } else { 0 };
        let bytes_mut = bytes.as_mut();
        bytes_mut[header_offset as usize..(header_offset as usize) + 1]
            .copy_from_slice(&page_kind.as_byte().to_be_bytes());
        let mut page = Self {
            page_no,
            page_size,
            bytes,
            usable_size,
            header_offset,
        };
        page.reset_for_rebuild()?;
        Ok(page)

        // a recycled frame is not guaranteed to be zeroed, so the cell count
        // has to be written out too instead of relying on the old bytes
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        self.bytes.as_mut()
    }

    fn set_u8_at(&mut self, o: usize, v: u8) {
        self.bytes_mut()[o..o + 1].copy_from_slice(&v.to_be_bytes());
    }
    fn set_u16_at(&mut self, o: usize, v: u16) {
        self.bytes_mut()[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn set_u32_at(&mut self, o: usize, v: u32) {
        self.bytes_mut()[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    /// Write the page's type byte.
    pub fn set_page_type(&mut self, page_type: BTreePageType) -> InkResult<()> {
        let byte = page_type.as_byte();
        let offset = self
            .downgrade()?
            .with_header_offset(BTREE_TYPE_PAGE_OFFSET as _);
        self.set_u8_at(offset, byte);
        Ok(())
    }
    fn set_no_of_cells(&mut self, n: u16) -> InkResult<()> {
        let o = self.downgrade()?.with_header_offset(CELL_COUNT_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    fn set_cell_content_area(&mut self, n: u16) -> InkResult<()> {
        let o = self
            .downgrade()?
            .with_header_offset(CELL_CONTENT_AREA_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    fn set_frag_cnt(&mut self, n: u8) -> InkResult<()> {
        let o = self
            .downgrade()?
            .with_header_offset(FRAGMENTED_FREE_BYTES_OFFSET);
        self.set_u8_at(o, n);
        Ok(())
    }
    fn set_first_freeblock(&mut self, n: u16) -> InkResult<()> {
        let o = self.downgrade()?.with_header_offset(FIRST_FREEBLOCK_OFFSET);
        self.set_u16_at(o, n);
        Ok(())
    }
    /// Write the right-most child pointer, which only an interior page has.
    pub fn set_right_most_ptr(&mut self, r: u32) -> InkResult<()> {
        assert_with_runtime_err(self.downgrade()?.page_type()?.is_interior(), || {
            "SetRightMostPointer called on a leaf".into()
        })?;
        let o = self
            .downgrade()?
            .with_header_offset(RIGHT_MOST_POINTER_OFFSET);
        self.set_u32_at(o, r);
        Ok(())
    }
    /// Clear the page so it can be filled again: no cells, no freeblocks, and
    /// the content area back at the end of the page.
    pub fn reset_for_rebuild(&mut self) -> InkResult<()> {
        self.set_cell_content_area(self.usable_size as _)?;
        self.set_first_freeblock(0)?;
        self.set_frag_cnt(0)?;
        self.set_no_of_cells(0)?;
        if self.is_interior()? {
            self.set_right_most_ptr(0)?;
        }
        Ok(())
    }

    /// A read-only view over the same bytes, for the code that works through a
    /// shared reference.
    fn downgrade(&mut self) -> InkResult<PageRef<'_>> {
        let header_len = if self.page_no == 1 {
            self.header_offset as usize
        } else {
            0
        };
        BTreePage::<&[u8]>::new(
            self.page_no,
            self.page_size,
            self.usable_size,
            header_len,
            &*self.bytes_mut(),
        )
    }
    /*

     * Claim `size` bytes from the freelist (first-fit).
     * - leftover == 0: unlink the whole block.
     * - 0 < leftover < 4: unlink the block, crumbs go to frag_cnt (too small
     * to form a freeblock).
     * - leftover >= 4: carve `size` bytes off the front, the remainder stays
     * a freeblock at [offset + size] with the old next pointer; prev (or
     * the header when taking from the head) is relinked to it.

    */
    /// Take the first freeblock with room for `size` bytes, answering with where
    /// it is, or nothing when no freeblock is big enough.
    ///
    /// A leftover under four bytes has no room to be a freeblock of its own, so
    /// it goes into the fragmented count; a larger one stays a freeblock in
    /// place.
    pub fn get_freeblock(&mut self, size: u16) -> InkResult<Option<u16>> {
        let page_ref = self.downgrade()?;
        if page_ref.first_freeblock()? == 0 {
            return Ok(None);
        }
        let mut prev: Option<u16> = None;
        let mut current = page_ref.first_freeblock()?;
        while current != 0 {
            let block = FreeCell::parse(current, page_ref.bytes())?;
            if block.size >= size {
                let leftover = block.size - size;
                if leftover < 4 {
                    // Unlink the whole block; crumbs (<4) become fragmentation.
                    match prev {
                        None => {
                            self.set_first_freeblock(block.next)?;
                        }
                        Some(prev_off) => {
                            let prev_off = prev_off as usize;
                            self.bytes_mut()[prev_off..prev_off + 2]
                                .copy_from_slice(&block.next.to_be_bytes());
                        }
                    }
                    if leftover > 0 {
                        let new_frag_cnt =
                            self.downgrade()?.frag_cnt()?.saturating_add(leftover as _);
                        self.set_frag_cnt(new_frag_cnt)?;
                    }
                } else {
                    // Split: caller takes [offset, offset + size), remainder
                    // stays a freeblock at offset + size.
                    let new_off = current + size;
                    let new_off_usize = new_off as usize;
                    self.bytes_mut()[new_off_usize..new_off_usize + 2]
                        .copy_from_slice(&block.next.to_be_bytes());
                    self.bytes_mut()[new_off_usize + 2..new_off_usize + 4]
                        .copy_from_slice(&leftover.to_be_bytes());
                    match prev {
                        None => {
                            self.set_first_freeblock(new_off)?;
                        }
                        Some(prev_off) => {
                            let prev_off = prev_off as usize;
                            self.bytes_mut()[prev_off..prev_off + 2]
                                .copy_from_slice(&new_off.to_be_bytes());
                        }
                    }
                }
                return Ok(Some(block.starting_offset));
            }
            prev = Some(current);
            current = block.next;
        }
        Ok(None)
    }
    /// Put a cell into the page.
    ///
    /// It takes room from a big enough freeblock when there is one, and
    /// otherwise from the top of the content area, defragmenting first when the
    /// room is there but scattered. When the cell simply does not fit, nothing
    /// changes and the answer is None.
    pub fn insert_cell<T: AsRef<[u8]>>(
        &mut self,
        content: &T,
        cell_idx: CellIndex,
    ) -> Result<InsertionState, InkError> {
        let content = content.as_ref();
        let page = self.downgrade()?;
        let gap = page
            .cell_content_area()?
            .saturating_sub(page.header_size()? as u16 + page.no_of_cells()? * 2)
            as usize;
        if gap >= 2
            && let Some(offset) = self.get_freeblock(content.as_ref().len() as _)?
        {
            self.insert_cell_at(content, offset as usize, cell_idx, false)?;
            return Ok(InsertionState::Inserted);
        }
        let page = self.downgrade()?;
        // If no freeblock can fit the content but there is enough total free space,
        // the space is too fragmented, so we need to rebuild the page.
        if page.remaining_space()? < content.len() + 2 {
            // we need to defragment it.
            if page.freespace()? >= content.len() + 2 {
                self.defragment()?;
                let result = self.insert_cell(&content, cell_idx)?;
                assert_with_internal_err(matches!(result, InsertionState::Inserted), || {
                    "Cell does not fite even after defragementation".into()
                })?;
                return Ok(result);
            }
            return Ok(InsertionState::None); // overflow
        }
        let offset = self.downgrade()?.cell_content_area()? as usize - content.len();
        self.insert_cell_at(content, offset, cell_idx, true)?;
        Ok(InsertionState::Inserted)
    }
    /// Pack the page's cells together, merging the scattered gaps into one
    /// contiguous free space that can be reused.
    fn defragment(&mut self) -> InkResult<()> {
        let page = self.as_ref()?;
        let mut cells = Vec::new();
        let mut cells_len = Vec::new();
        for i in 0..page.no_of_cells()? {
            let cell = page.cell_bytes_as_ref(i)?;
            cells.extend_from_slice(cell);
            cells_len.push(cell.len());
        }

        self.set_cell_content_area(self.usable_size as _)?;
        self.set_first_freeblock(0)?;
        self.set_frag_cnt(0)?;
        self.set_no_of_cells(0)?;
        let mut start = 0;
        for (i, &len) in cells_len.iter().enumerate() {
            let bytes = &cells[start..len + start];
            self.insert_cell(&bytes, i as _)?;
            start += len;
        }
        Ok(())
    }

    /// Write a cell at a given offset and slot its pointer into the array.
    /// `from_top` says whether the content area boundary moves up to account for
    /// the new bytes.
    fn insert_cell_at(
        &mut self,
        content: &[u8],
        offset: usize,
        i: u16,
        from_top: bool, /*Is it from the freelist or not*/
    ) -> InkResult<()> {
        self.bytes_mut()[offset..offset + content.len()].copy_from_slice(content);
        let arr = self.downgrade()?.header_size()? as u16;
        let n = self.downgrade()?.no_of_cells()?;
        let (from, to) = ((arr + 2 * i) as usize, (arr + 2 * n) as usize);
        self.bytes_mut().copy_within(from..to, from + 2);
        self.set_u16_at(from, offset as u16);
        self.set_no_of_cells(n + 1)?;
        if from_top {
            let cca = self.downgrade()?.cell_content_area()? as usize;
            self.set_cell_content_area((cca - content.len()) as _)?;
        }
        Ok(())
    }
    /// Copy another page's content into this one, shifting the cell pointers
    /// when the two pages keep their headers at different offsets.
    pub fn copy_data_from(&mut self, other: &Self) -> Result<(), InkError> {
        if self.usable_size != other.usable_size || self.bytes_mut().len() < other.usable_size {
            return Err(InkError::Internal(
                "copy_data_from between pages with different usable sizes",
            ));
        }
        let (old_ho, new_ho) = (other.header_offset as usize, self.header_offset as usize);
        self.bytes_mut()[new_ho..other.usable_size]
            .copy_from_slice(&other.bytes.as_ref()[old_ho..other.usable_size]);
        let delta = new_ho as isize - old_ho as isize;
        if delta != 0 {
            let arr = self.downgrade()?.header_size()? as usize;
            let n = self.downgrade()?.no_of_cells()?;
            for i in 0..n {
                let p = self.cell_ptr(i)?;
                self.set_u16_at(arr + 2 * i as usize, (p as isize + delta) as u16);
            }
        }
        Ok(())
    }
    /// A read-only view of the page.
    pub fn as_ref(&self) -> InkResult<PageRef<'_>> {
        let header_len = if self.page_no == 1 {
            self.header_offset as usize
        } else {
            0
        };
        BTreePage::<&[u8]>::new(
            self.page_no,
            self.page_size,
            self.usable_size,
            header_len,
            self.bytes.as_ref(),
        )
    }
    /// A view of the page, narrowed to a shared one.
    pub fn as_mut_view(&mut self) -> InkResult<PageRef<'_>> {
        self.downgrade()
    }
    /// Put a run of free bytes back into the page.
    ///
    /// The run is spliced into the freeblock list, merging with a neighbour when
    /// it sits right against one, and becoming the new head when it lies before
    /// the block that is currently first.
    pub fn insert_freeblock(&mut self, offset: usize, size: usize) -> InkResult<()> {
        let hdr = self.downgrade()?.header_size()? as usize;
        assert_with_corrupt_err(offset >= hdr && offset + size <= self.usable_size, || {
            format!(
                "freeblock [{offset}, {}) outside content area",
                offset + size
            )
        })?;
        debug_assert!(size >= 4, "Freeblock size cannot be less than 4 bytes");

        // CASE [A]: There are no freeblocks yet.
        if self.downgrade()?.first_freeblock()? == 0 {
            self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);
            self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());

            let start = self.header_offset as usize + FIRST_FREEBLOCK_OFFSET;
            let end = start + FIRST_FREEBLOCK_SIZE;

            self.bytes_mut()[start..end].copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        let first_cell = FreeCell::parse(self.downgrade()?.first_freeblock()?, self.bytes_mut())?;

        // CASE [B]: offset comes before the current first freeblock.
        // This must be checked BEFORE anything else, since every later
        // branch assumes [`offset`] only ever increases relative to the
        // node it's being compared against.
        if offset + size <= first_cell.starting_offset as usize {
            let new_end = offset + size;

            // [NEW][FIRST]  (adjacent -> merge)
            if new_end == first_cell.starting_offset as usize {
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&first_cell.next.to_be_bytes());

                let new_size = size + first_cell.size as usize;
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());
            } else {
                // [NEW] ... [FIRST]  (gap -> just link, no merge)
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&(first_cell.starting_offset).to_be_bytes());

                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(size as u16).to_be_bytes());
            }

            // Either way NEW becomes the new head of the freelist.
            let start = self.header_offset as usize + FIRST_FREEBLOCK_OFFSET;
            let end = start + FIRST_FREEBLOCK_SIZE;
            self.bytes_mut()[start..end].copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        // CASE [C]: There is only one freeblock.
        if first_cell.next == 0 {
            // [A][NEW]
            if first_cell.starting_offset as usize + first_cell.size as usize == offset {
                let first_cell_offset = first_cell.starting_offset as usize;

                self.bytes_mut()[first_cell_offset + 2..first_cell_offset + 4]
                    .copy_from_slice(&((first_cell.size as usize + size) as u16).to_be_bytes());

                return Ok(());
            }

            // [A] ... [NEW]
            // (We already handled offset <= first_cell above, so here
            // offset is guaranteed to be strictly after A and non adjacent.)
            self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);
            self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());
            let first_cell_offset = first_cell.starting_offset as usize;
            self.bytes_mut()[first_cell_offset..first_cell_offset + 2]
                .copy_from_slice(&(offset as u16).to_be_bytes());

            return Ok(());
        }

        // Multiple freeblocks.
        let mut prev_cell = first_cell;

        while prev_cell.next != 0 {
            let current_cell = FreeCell::parse(prev_cell.next, self.bytes_mut())?;

            let current_cell_offset = current_cell.starting_offset as usize;
            let current_cell_size = current_cell.size as usize;

            let prev_cell_offset = prev_cell.starting_offset as usize;
            let prev_cell_end = prev_cell_offset + prev_cell.size as usize;

            let new_end = offset + size;

            // [PREV][NEW][CURRENT]
            if prev_cell_end == offset && new_end == current_cell_offset {
                let new_size = prev_cell.size as usize + size + current_cell_size;

                // PREV.size = PREV + NEW + CURRENT
                self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());

                // PREV.next = CURRENT.next
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&current_cell.next.to_be_bytes());

                return Ok(());
            }

            // [PREV][NEW] ... [CURRENT]
            if prev_cell_end == offset {
                let new_size = prev_cell.size as usize + size;

                self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                    .copy_from_slice(&(new_size as u16).to_be_bytes());

                return Ok(());
            }

            // [PREV] ... [NEW][CURRENT]
            if new_end == current_cell_offset {
                // NEW.next = CURRENT.next
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&current_cell.next.to_be_bytes());

                // NEW.size = NEW + CURRENT
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&((size + current_cell_size) as u16).to_be_bytes());

                // PREV.next = NEW
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&(offset as u16).to_be_bytes());

                return Ok(());
            }

            // [PREV] ... [NEW] ... [CURRENT]
            // We've already ruled out offset <= first_cell, and the loop guarantees
            // offset > prev_cell_end here.
            if offset > prev_cell_end && offset < current_cell_offset {
                // NEW.next = CURRENT
                self.bytes_mut()[offset..offset + 2]
                    .copy_from_slice(&(current_cell_offset as u16).to_be_bytes());

                // NEW.size = size
                self.bytes_mut()[offset + 2..offset + 4]
                    .copy_from_slice(&(size as u16).to_be_bytes());

                // PREV.next = NEW
                self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
                    .copy_from_slice(&(offset as u16).to_be_bytes());

                return Ok(());
            }

            prev_cell = current_cell;
        }

        // We reached the last freeblock.
        //
        // [PREV][NEW]
        if prev_cell.starting_offset as usize + prev_cell.size as usize == offset {
            let prev_cell_offset = prev_cell.starting_offset as usize;

            let new_size = prev_cell.size as usize + size;

            self.bytes_mut()[prev_cell_offset + 2..prev_cell_offset + 4]
                .copy_from_slice(&(new_size as u16).to_be_bytes());

            return Ok(());
        }

        // [PREV] ... [NEW]
        self.bytes_mut()[offset..offset + 2].copy_from_slice(&[0, 0]);

        self.bytes_mut()[offset + 2..offset + 4].copy_from_slice(&(size as u16).to_be_bytes());

        let prev_cell_offset = prev_cell.starting_offset as usize;

        self.bytes_mut()[prev_cell_offset..prev_cell_offset + 2]
            .copy_from_slice(&(offset as u16).to_be_bytes());

        Ok(())
    }
    /// How many bytes cell i takes.
    pub fn cell_size(&mut self, i: u16) -> InkResult<usize> {
        let s = self.downgrade()?;
        let cell_sp = s.cell_span(s.cell_ptr(i)?)?;
        Ok(cell_sp.end - cell_sp.start)
    }
    /// How many bytes the cell at an offset takes.
    pub fn cell_size_by_offset(&mut self, offset: u16) -> InkResult<usize> {
        assert!((offset as usize) < self.usable_size);
        let cell_sp = self.downgrade()?.cell_span(offset)?;
        Ok(cell_sp.end - cell_sp.start)
    }
    /// Take a cell out of the page: its bytes become a freeblock again and its
    /// pointer leaves the array.
    pub fn remove_cell(&mut self, cell_idx: CellIndex) -> InkResult<()> {
        let s = self.downgrade()?;
        let cell_ptr = s.cell_ptr(cell_idx)?;
        let cell_span = s.cell_span(cell_ptr)?;
        let bytes_len: usize = cell_span.end - cell_span.start;
        let n = s.no_of_cells()?;
        self.insert_freeblock(cell_ptr as _, bytes_len)?;
        self.remove_cell_pointer_entry(cell_idx)?;
        self.set_no_of_cells(n - 1)?;
        Ok(())
    }
    /// Shift the pointer array left over the slot a cell used to hold.
    fn remove_cell_pointer_entry(&mut self, i: u16) -> InkResult<()> {
        let n = self.downgrade()?.no_of_cells()? as usize;
        debug_assert!(n > i as usize, "Cell index out of the cell pointer array");
        // header_size() already includes header_offset — do not add it again.
        let arr = self.downgrade()?.header_size()? as usize;
        let (from, to) = (arr + 2 * (i as usize + 1), arr + 2 * n);
        self.bytes_mut().copy_within(from..to, from - 2);
        Ok(())
    }
    /// Replace a cell's bytes.
    ///
    /// If the new bytes do not fit, the page is left unchanged and `None` is
    /// returned.
    pub fn replace_cell(&mut self, i: u16, content: impl AsRef<[u8]>) -> InkResult<InsertionState> {
        let content = content.as_ref();
        let n = self.no_of_cells()? as usize;
        if i as usize >= n {
            return Err(InkError::InternalFmt(format!(
                "replace_cell: index {i} out of bounds (page holds {n} cells)"
            )));
        }
        // Check that it fits before changing anything. If it does not fit, leave
        // the page unchanged so the operation can be retried without losing the old cell.
        let mut bodies = content.len();
        for j in 0..n {
            if j != i as usize {
                bodies += self.cell_bytes_as_ref(j as u16)?.len();
            }
        }
        if bodies + n * 2 + self.header_size()? as usize > self.usable_size {
            return Ok(InsertionState::None);
        }
        let old = self.cell_bytes_as_ref(i)?.to_vec();
        self.remove_cell(i)?;
        match self.insert_cell(&content, i)? {
            InsertionState::Inserted => {
                // self.debug_check_child_pointers();
                Ok(InsertionState::Inserted)
            }
            InsertionState::None => match self.insert_cell(&old, i)? {
                InsertionState::Inserted => Ok(InsertionState::None),
                _ => Err(InkError::Corrupt(CorruptError::ReplaceCellRefused)),
            },
        }
    }
}
