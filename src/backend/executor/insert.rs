use std::collections::VecDeque;

use crate::backend::executor::Row;
use crate::backend::executor::eval::Eval;
use crate::backend::planner::plan::Plan;
use crate::errors::SqliteError;
use crate::format::page::PageNo;
use crate::pager::pager::Pager;
use crate::record::{Value, encode_sqlite};
use crate::sql::parser::ExprArena;
use crate::storage::btree::{BTree, BTreeCursor};
use crate::storage::cell::{BTreeCellType, Encode};
use crate::varint::encode_varint;
use crate::vfs::cursor;

#[derive(Debug)]
pub struct Insert {
    root_page: PageNo,
    values: Vec<Value<'static>>,
    hint: Option<u64>,
}

impl Insert {
    pub fn new(root_page: PageNo, values: Vec<Value<'static>>, hint: Option<u64>) -> Self {
        Self {
            root_page,
            values,
            hint,
        }
    }

    pub fn next(&self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        let mut payload = Vec::<u8>::new();
        let mut header = Vec::<u8>::new();
        let mut buffer = [0u8; 9];
        for value in self.values.iter() {
            let data_type = encode_sqlite(value, &mut payload);
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

        let mut cursor = BTreeCursor::new(self.root_page);
        cursor.last(pager)?;
        let (page_no, cell_idx) = cursor.last_visited_entry().unwrap_or((self.root_page, 0));
        let guard = pager.get(page_no)?;
        let row_id = (BTreeCursor::page_as_ref(page_no, &guard, pager)?)
            .cell(cell_idx)?
            .row_id();

        let payload = Encode::encode_cell(BTreeCellType::TableLeaf, header, (row_id as u32) + 1);
        let mut btree = BTree::new(self.root_page, pager);

        btree.insert(payload, page_no, cell_idx + 1)?;
        Ok(None)
    }
}
