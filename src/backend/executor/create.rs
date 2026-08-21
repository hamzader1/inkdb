use crate::backend::analyze::ResolvedCreateTableQuery;
use crate::errors::SqliteError;
use crate::pager::pager::Pager;
use crate::record::{SqlType, Value, encode_sqlite};
use crate::storage::btree::{BTree, BTreeCursor};
use crate::storage::cell::Encode;
use crate::storage::page::{BTreePageMut, BTreePageType};
use crate::varint::encode_varint;

use super::Row;
use super::insert::Insert;

#[derive(Debug)]
pub struct CreateTable {
    meta: ResolvedCreateTableQuery,
}
impl CreateTable {
    pub fn new(meta: ResolvedCreateTableQuery) -> Self {
        Self { meta }
    }

    pub fn next(&self, pager: &mut Pager) -> Result<Option<Row>, SqliteError> {
        let name = &self.meta.meta.name;
        let mut temp_btree = BTree::new(1, pager);
        let new_page = temp_btree.allocate_page()?;
        let mut guard = pager.get_mut(new_page)?;
        let bytes = guard.bytes_as_mut().unwrap();

        let page = BTreePageMut::new_from_scratch(
            new_page,
            BTreePageType::LeafTable,
            bytes,
            pager.metadata.page_size,
            pager.metadata.usable_size,
        );
        let row = [
            Value::text("table"),
            Value::text(name),
            Value::text(name),
            Value::Integer(new_page as _),
            Value::text(self.meta.meta.query.as_ref()),
        ];

        // OPTIMAZE THIS BY ABSTRACTING INSERT FUNCTION
        let insert = Insert::new(1, row.into_iter().map(|a| a.into_owned()).collect(), None);
        insert.next(pager)?;
        Ok(None)
        // let mut payload = Vec::<u8>::new();
        // let mut header = Vec::<u8>::new();
        // let mut buffer = [0u8; 9];
        // for value in row.iter() {
        //     let data_type = encode_sqlite(value, &mut payload);
        //     let vint = encode_varint(&mut buffer, data_type as _);
        //     header.extend_from_slice(&buffer[..vint]);
        // }
        // let len = encode_varint(&mut buffer, header.len() as _); // 1byte
        // let with_len = encode_varint(&mut buffer, len as u64 + header.len() as u64);
        // let v_b = &buffer[..with_len];
        // for byte in v_b.iter().rev() {
        //     header.insert(0, *byte);
        // }
        // header.extend_from_slice(&payload);

        // let mut cursor = BTreeCursor::new(1);
        // let is_empty_leaf = cursor.last(pager)?;
        // let (page_no, cell_idx) = cursor.last_visited_entry().unwrap();
        // let guard = pager.get(page_no)?;
        // dbg!(cursor.current(pager));
        // let mut btree = BTree::with_cursor(pager, cursor);
        // let next_row_id = if is_empty_leaf {
        //     1
        // } else {
        //     (btree.page_as_ref(page_no, &guard)?.cell(cell_idx)?.row_id() + 1)
        // };
        // let payload = Encode::encode_table_leaf_cell(header, next_row_id as _);

        // btree.insert(next_row_id.into_sqlite_value(), payload)?;
        // Ok(None)
    }
}
