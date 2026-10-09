use crate::InkResult;
use crate::pager::pager::{PageNo, Pager};
use crate::record::Value;
use crate::storage::btree::kind::TypedPage;
use crate::storage::page::{BTreePage, PageMut, PageRef};
use crate::vfs::Vfs;

use super::kind::{Cell, HasPayload, PageKind};
use super::{BTreeCursor, RestorePosition, SeekResult, page_as_ref_with_pager};

/// Data in a database file can be organized in several ways, such as entry
/// sequence, relative order, hashing, or key sequence. SQLite uses B+ trees to
/// organize table data and B trees to organize indexes. Both are key sequence
/// data structures.
///
/// Here, we discuss the implementation of B and B+ trees commonly used for
/// ordered indexes in disk based databases. The algorithm implemented here is
/// not a one to one match with SQLite's implementation.
///
/// A B tree, is one of the most important index structures used in external storage based DBMSs.
/// It organizes a collection of similar records in sorted order by their keys. Different B trees in the
/// same database can use different sort orders.
///
/// A B tree is a height balanced n ary tree, where n > 2 and all leaf nodes are
/// at the same level. Entries and search information, such as key values, are
/// stored in both internal and leaf nodes.
///
/// B trees provide near optimal performance for insertion, deletion, search,
/// and search next operations, with a time complexity of `O(log N)`.
///
///
/// A B+tree is a variant of a Btree where all entries are stored in the leaf
/// nodes as key value pairs. Internal nodes contain only keys and child pointers
/// used to route searches.
///
/// Internal nodes can have a variable number of children within a fixed range.
/// All leaf nodes are at the same level, and they may be linked together for
/// ordered traversal. The root is always an internal node.
///
/// For an n + 1 ary tree, an internal node can contain at most n keys and
/// n + 1 child pointers. The keys divide the child subtrees into ranges, guiding
/// the search to the appropriate child.
///
/// Searching for a key takes O(log m) node traversals, where m is the number of
/// entries in the tree.
/// ```text
///    +------+------+------+------+-----------------+--------+--------+----------+
///    | Ptr0 | Key1 | Ptr1 | Key1 |       ...       |Ptr n-1 |Key n-1 |  Ptr n   | RMP
///    +------+------+------+------+-----------------+--------+--------+----------+
///        /            /                                 /                  \
///       /            /                                 /                    \
///   /--/         /--/                              /--/                      \--\
///  /            /                                 /                              \
/// v            v                                 v                                v
///
/// ```
/// From now on, we will use the following tree diagrams to make the structure
/// and operations easier to understand.
///
/// ```text
///                                                                  ROOT PAGE
///
///                                       HEADER                 +---------------------+
///                                                              |                     |
///                                    +---------------------+---|---------------------v-------+
///                                    |+-------+ +--+ +---+ |+----+                +----+---+ |
///                                    || 2 | 5 | |NC| |RMP| || P1 | ->    RS    <- | LC | D | |        INTERIOR
///                                    |+-------+ +--+ +-*-+ |+----+                +----+---+ |
///                                    +-----------------+---+-------------------------+-------+
///                                                      |                             |
///                                                      |                             |
///                                       +--------------+-----------------------------+
///                                       |              |
///                                       |              +-----------------------------------------------------+
///                                       |                                                                    |
///                                       |                                                                    |
///                                       |                                                                    |
///                       CELL POINTER    |                                                                    |
///                          ARRAY        v                   CELLS                                            v
///      +---------------+-------------------------------------------+         +---------------+----------------------------------------------+
///      |+-------+ +--+ |+----+ +----+                +---+  +---+  |         |+-------+ +--+ |+----+ +----+                 +---+  +---+---+|
/// LEAF ||10 | 13| |NC| || P1 | | P2 | ->   RS    <-  | D |  | D |  |         ||10 | 13| |NC| || P1 | | P2 | ->   RS    <-   | D |  | D |OVP||  LEAF
///      |+-------+ +--+ |+----+ +----+                +---+  +---+  |         |+-------+ +--+ |+----+ +----+                 +---+  +---+---+|
///      +---------------+---+------|--------------------^------^----+         +---------------+---+------|---------------------^------^---+--+
///                          |      |                    |      |                                  |      |                     |      |   |
///                          |      +--------------------+      |                                  |      +---------------------+      |   |
///                          |                                  |                                  |                                   |   |
///                          +----------------------------------+                                  +-----------------------------------+   |
///                                                                                                                                        |
///                                         +----------------------------------------------------------------------------------------------+
///                                         |
///                                         |
///                                         |
///                                         v
///                                     +-----+-------------------------------------------+
///                                     | NEXT|OVERFLOW PAYLOAD                           |
///                                     +-----+-------------------------------------------+
///                                         |
///                                         |
///                                         |                                            +----------------------------+
///                                         +------------------------------------------->|   ANOTHER [OVP] OR NULL    |
///                                                                                      +----------------------------+
///
/// ```
/// Here's what everything stands for:
/// ```text
/// +------+--------------------------------------+
/// | Code | Meaning                              |
/// +------+--------------------------------------+
/// | 2    | Internal index tree page             |
/// | 5    | Internal table tree page             |
/// | 10   | Leaf index tree page                 |
/// | 13   | Leaf table tree page                 |
/// | NC   | Number of cells                      |
/// | LC   | Left child                           |
/// | RMP  | Right most pointer                   |
/// | RS   | Remaining space                      |
/// | D    | Data                                 |
/// | P1   | Pointer 1                            |
/// | P2   | Pointer 2                            |
/// | Pn   | Pointer n                            |
/// | OVP  | Overflow page                        |
/// +------+--------------------------------------+
/// ```
///
/// # Example of a table BTree
/// ``` text
///                                        PageNo(2)
///                                                  +-----------+
///                                                 /|    100    |\
///                                                / +-----------+ \
///                             /-----------------/                 \-------------------\
///          PageNo(3)         /                                                         \
///                           v                                                           v        Interior page ( PageNo(4) )
///                     +-----------+                                               +-----------+  and RMP of Page(2)
///                    /|  40, 80   |\                                             /| 120, 190  |\
///                   / +-----------+ \                                           / +-----------+ \
///         /--------/        |        \---------\                        /------/        |        \-------------\
///        /                  |                   \                      /                |                       \
///       v                   v                    v                    v                 v                        v
/// +-----------+     +---------------+    +---------------+     +-------------+   +------------------+  +-------------------+
/// |10, 20, 30 |     |42, 51, 60, 70 |    |82, 91, 97, 97 |     |101, 102,119 |   |122, 123, 140, 150|  |193, 199, 250, 299 |
/// +-----------+     +---------------+    +---------------+     +-------------+   +------------------+  +-------------------+
///                                          RMP of Page(3)                                                  RMP of Page(4)
/// ```
/// If you plan to build a B+ tree for your own database (or anything), it is better to start
/// by building one in memory, since it is MUCH easier than building one in disk.
/// If you are interested, I built one before:
/// [`Source`](https://github.com/hamzader1/bptree)
///
pub struct BTree<'a, V: Vfs> {
    pub(crate) root_page: PageNo,
    pub(crate) pager: &'a mut Pager<V>,
    pub(crate) cursor: BTreeCursor<V>,
}

impl<'a, V: Vfs> BTree<'a, V> {
    pub fn with_cursor(pager: &'a mut Pager<V>, cursor: BTreeCursor<V>) -> Self {
        Self {
            root_page: cursor.root,
            pager,
            cursor,
        }
    }

    /// Store a value under a key. The bytes are a whole cell, already laid out.
    pub fn insert(&mut self, key: &Value, content: Vec<u8>) -> InkResult<()> {
        self.insert_cell(key, content)
    }

    /// Remove a key, answering whether it was there.
    pub fn delete(&mut self, key: Value) -> InkResult<bool> {
        self.delete_value(&key)
    }

    /// Move to a key, stopping at the leaf that holds it.
    pub fn seek(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek(self.pager, target)
    }

    /// Move to the first entry that is not less than the key.
    pub fn seek_lower_bound(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek_lower_bound(self.pager, target)
    }

    /// Move to a key, stopping on an interior page when the key is found there,
    /// since that is the page an index divider is deleted from.
    pub fn seek_for_delete(&mut self, target: &Value) -> InkResult<SeekResult> {
        self.cursor.seek_for_delete(self.pager, target)
    }
    /// The cell the cursor is on, or nothing when it has run past the cells of
    /// the page it is on.
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

    /// Step to the next entry, going up when the page runs out of cells.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> InkResult<()> {
        self.cursor.next(self.pager)
    }

    /// Step back to the previous entry.
    pub fn prev(&mut self) -> InkResult<()> {
        self.cursor.prev(self.pager)
    }

    /// Move to the smallest entry in the tree.
    pub fn first(&mut self) -> InkResult<()> {
        self.cursor.first(self.pager)
    }

    /// Move to the largest entry in the tree.
    pub fn last(&mut self) -> InkResult<()> {
        self.cursor.last(self.pager)
    }

    /// Step forward until the cursor is on a real cell, or past the end of the
    /// tree.
    ///
    /// A seek that finds nothing lands on the slot the key would have gone in,
    /// which is not a cell the cursor can yield.
    pub fn skip_past_end(&mut self) -> InkResult<()> {
        self.cursor.skip_past_end(self.pager)
    }

    /// Remember where the cursor is as a key, and let go of the pages.
    ///
    /// Pages cannot be held between two statements, so the position is kept as
    /// the key of the current cell and found again with a seek.
    pub fn save_position(&mut self) -> InkResult<()> {
        self.cursor.save_position(self.pager)
    }

    /// Find again where the cursor was. A key that has left the tree in the
    /// meantime leaves the cursor on the entry that took its place.
    #[allow(dead_code)]
    pub(crate) fn restore_position(&mut self) -> InkResult<RestorePosition> {
        self.cursor.restore_position(self.pager)
    }
    /// The largest row id in the tree, which sits in the last cell of the last leaf.
    pub fn max_row_id(&mut self) -> InkResult<u64> {
        self.cursor.max_row_id(self.pager)
    }

    /// The values of the row the cursor is on.
    pub fn current_record<K: PageKind>(&mut self) -> InkResult<Option<Vec<Value<'_>>>>
    where
        K::Cell: HasPayload + Cell,
    {
        self.cursor.current_record::<K>(self.pager)
    }

    /// How many cells the page the cursor last landed on holds.
    pub fn current_page_header_unchecked(&mut self) -> InkResult<u16> {
        let (page_no, _) = self.cursor.last_visited_entry_unchecked();
        self.with_page_ref(page_no, |page| page.no_of_cells())
    }

    /// The page for something new: one off the freelist when there is one, and
    /// otherwise a page grown at the end of the file.
    pub fn allocate_page(&mut self) -> InkResult<PageNo> {
        self.pager.allocate_new_page()
    }

    /// Put a page back on the freelist for something else to use.
    pub fn deallocate_page(&mut self, page_no: PageNo) -> InkResult<()> {
        self.pager.dealloc(page_no)
    }

    /// Read a page and look at it through a function.
    pub fn with_page_ref<F, R>(&mut self, page_no: PageNo, f: F) -> InkResult<R>
    where
        F: FnOnce(&PageRef<'_>) -> InkResult<R>,
    {
        let guard = self.pager.get(page_no)?;
        let page = page_as_ref_with_pager(page_no, &guard, self.pager)?;
        f(&page)
    }

    /// The same for a page that is about to be changed.
    pub fn with_page_mut<F, R>(&mut self, page_no: PageNo, f: F) -> InkResult<R>
    where
        F: FnOnce(&mut PageMut<'_>) -> InkResult<R>,
    {
        let mut guard = self.pager.get_mut(page_no)?;
        let mut page = super::page_as_mut_with_pager(page_no, &mut guard, self.pager)?;
        f(&mut page)
    }
}
