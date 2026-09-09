use crate::{
    SqliteResult,
    backend::{
        executor::{Row, eval::Eval},
        planner::plan::Plan,
    },
    pager::pager::Pager,
    record::Value,
    sql::parser::ExprArena,
    storage::btree::{BTree, SeekResult},
    vfs::file::SqliteFile,
};

struct IndexExactMatch<F: SqliteFile> {
    child: Plan<F>,
    index_root_page: u32,
    relation_root_page: u32,
    key: Value<'static>,
    target: usize,
}

impl<F: SqliteFile> IndexExactMatch<F> {
    fn new(
        child: Plan<F>,
        index_root_page: u32,
        relation_root_page: u32,
        key: Value<'static>,
        target: usize,
    ) -> Self {
        Self {
            child,
            index_root_page,
            relation_root_page,
            key,
            target,
        }
    }
    pub fn next(&mut self, pager: &mut Pager<F>, arena: &ExprArena) -> SqliteResult<Option<Row>> {
        let target = Eval::eval(arena, self.target, None)?;
        let mut index_btree = BTree::new(self.index_root_page, pager);
        let seek_res = index_btree.search(target)?;
        if seek_res == SeekResult::NotFound {
            return Ok(None);
        }
        let current_index_record = index_btree.cursor.current_record(pager)?;
        if let Some(mut index_record) = current_index_record {
            let row_id = index_record.pop().expect("Index record is empty");
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
            return Ok(Some(row));
        }

        Ok(None)
    }
}
