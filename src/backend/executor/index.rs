use crate::{
    SqliteResult,
    backend::{
        executor::{Row, eval::Eval},
        planner::plan::Plan,
    },
    errors::SqliteError,
    pager::pager::Pager,
    record::Value,
    sql::parser::ExprArena,
    storage::btree::{BTree, BTreeCursor, SeekResult},
    vfs::file::SqliteFile,
};

#[derive(Debug)]
pub struct IndexExactMatch<F: SqliteFile> {
    // child: Box<Plan<F>>,
    index_root_page: u32,
    relation_root_page: u32,
    target: Value<'static>,
    cursor: BTreeCursor<F>,
    is_done: bool,
}

impl<F: SqliteFile> IndexExactMatch<F> {
    pub fn new(
        pager: &mut Pager<F>,
        index_root_page: u32,
        relation_root_page: u32,
        target: Value<'static>,
    ) -> Result<Self, SqliteError> {
        let mut cursor = BTreeCursor::new(index_root_page);
        let seek_res = cursor.seek(pager, Value::Tuple(vec![target.clone()]))?;
        if seek_res == SeekResult::Exact
            && let Some(p) = cursor.stack.last_mut()
        {
            p.yeilded = true;
        }
        Ok(Self {
            index_root_page,
            relation_root_page,
            target,
            cursor,
            is_done: seek_res == SeekResult::NotFound,
        })
    }
    pub fn next(&mut self, pager: &mut Pager<F>, arena: &ExprArena) -> SqliteResult<Option<Row>> {
        if self.is_done {
            return Ok(None);
        }
        let current_index_record = self.cursor.current_record(pager)?;
        if let Some(mut index_record) = current_index_record {
            let row_id = index_record.pop().expect("Index record is empty");
            if index_record != vec![self.target.clone()] {
                self.is_done = true;
                return Ok(None);
            }
            let mut relation_btree = BTree::new(self.relation_root_page, pager);
            relation_btree.search(row_id.clone());
            let relation_record = relation_btree
                .cursor
                .current_record(pager)?
                .expect("Row id not associated with any record")
                .iter()
                .map(|v| v.into_owned())
                .collect();

            let row = Row::new(row_id.get_int()? as _, relation_record);
            self.cursor.next(pager)?;
            return Ok(Some(row));
        }
        self.is_done = true;
        Ok(None)
    }
}
