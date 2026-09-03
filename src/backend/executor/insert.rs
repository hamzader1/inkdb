use std::collections::VecDeque;

use crate::backend::executor::Row;
use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::pager::pager::{PageNo, Pager};
use crate::record::{SqlType, Value, tuple::Tuple};
use crate::sql::parser::ExprArena;
use crate::storage::btree::{BTree, BTreeCursor};
use crate::storage::cell::{BTreeCellType, Encode};
use crate::varint::encode_varint;
use crate::vfs::cursor;
use crate::vfs::file::SqliteFile;

#[derive(Debug)]
pub struct Insert<'a, F: SqliteFile> {
    root_page: PageNo,
    values: Vec<Vec<Value<'a>>>,
    hint: Option<u64>,
    _phantom: std::marker::PhantomData<F>,
}

impl<'a, F: SqliteFile> Insert<'a, F> {
    pub fn new(root_page: PageNo, values: Vec<Vec<Value<'a>>>, hint: Option<u64>) -> Self {
        Self {
            root_page,
            values,
            hint,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn next(&self, pager: &mut Pager<F>) -> Result<Option<Row>, SqliteError> {
        let mut btree = BTree::new(self.root_page, pager);
        btree.seek_into_last()?;
        let is_empty = btree.current_page_header_unchecked()?.no_of_cells == 0;
        let (page_no, cell_idx) = btree.cursor.last_visited_entry_unchecked();
        let next_row_id = if is_empty {
            1
        } else {
            btree.with_page_ref::<_, u64>(page_no, |page| Ok(page.cell(cell_idx)?.row_id()))? + 1
        };
        self.insert_batch(&mut btree, next_row_id)?;
        Ok(None)
    }
    fn insert_batch(&self, btree: &mut BTree<F>, mut row_id: u64) -> Result<(), SqliteError> {
        for inner_values in self.values.iter() {
            let mut header = Vec::<u8>::new();
            let mut payload = Vec::<u8>::new();
            let mut buffer = [0u8; 9];
            for value in inner_values.iter() {
                let data_type = Tuple::encode_sqltype(value, &mut payload);
                let vint = encode_varint(&mut buffer, data_type as _);
                header.extend_from_slice(&buffer[..vint]);
            }
            let len = encode_varint(&mut buffer, header.len() as _); // 1byte
            let with_len = encode_varint(&mut buffer, len as u64 + header.len() as u64);
            let v_b = &buffer[..with_len];
            for byte in v_b.iter().rev() {
                header.insert(0, *byte);
            }
            header.extend_from_slice(&payload);
            let cell_payload = Encode::encode_table_leaf_cell(header, row_id as _);
            btree.insert(row_id.into_sqlite_value(), cell_payload)?;
            row_id += 1;
        }
        Ok(())
    }
}
