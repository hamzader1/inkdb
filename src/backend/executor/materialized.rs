use crate::{
    InkResult,
    backend::{
        executor::{Row, context::ExecCtx, encode_frame, frame_len},
        planner::plan::Plan,
    },
    vfs::Vfs,
};

use crate::vfs::file::InkFile;

use super::StreamSource;

use super::MEM_CAP;
const FILE: &str = "ink_update";

/*

* MaterializedResult plan was created to be used mainly for the update plan.
* Update plan needs to (in plan) delete a row and insert it
* while some sort of scanner is yielding rows to it.
* However, this method won’t work well since there is a risk
* of getting stuck in an infinite loop, for example, we delete row A
* and insert row A, but the source of the scan (either index scan or tablescan)
* will continue searching for rows that match the where clause, this may lead to visiting
* the row twice or even more (infinity).
*
* As a solution to this, we will collect all rows either in memory buffer for smaller outputs or in disk
* in case if we exceed the maximum memory capacity.
* So later we start yielding rows safely since we have a copy of them stored in a
* StreamSouce
    */
#[derive(Debug)]
pub struct MaterializedResult<V: Vfs> {
    child: Box<Plan<V>>,
    stream_source: StreamSource<V>,
    stream_backup: Option<V::File>,
    nread: usize,
    rowid_column: Option<usize>,
    rowid_captured: bool,
}

impl<V: Vfs> MaterializedResult<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self {
            child,
            stream_source: StreamSource::None,
            stream_backup: None,
            nread: 0,
            rowid_column: None,
            rowid_captured: false,
        }
    }
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if let StreamSource::None = self.stream_source {
            self.collect(ctx)?;
        }
        let res = self
            .stream_source
            .yield_from_stream(&mut self.nread, self.rowid_column);
        if res.as_ref().is_ok_and(|opt| opt.is_none()) {
            let _ = ctx.pager.vfs_mut().remove_temp(FILE);
        }
        res
    }
    fn collect(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let mut buffer = Vec::new();
        let mut in_disk_data = false;
        let mut nrows = 0;
        let mut max_frame = 0;
        while let Some(row) = self.child.next(ctx)? {
            if !self.rowid_captured {
                self.rowid_column = row.rowid_column();
                self.rowid_captured = true;
            }
            nrows += 1;
            let row_bytes = row.stored_bytes().unwrap();
            let frame = frame_len(row_bytes.len(), row.key());
            max_frame = max_frame.max(frame);
            if frame + buffer.len() > MEM_CAP {
                match self.stream_backup {
                    Some(ref mut file) => {
                        file.write_all(&buffer)?;
                    }
                    None => {
                        let mut file = ctx.pager.vfs_mut().open_temp(FILE)?;
                        file.set_len(0)?;
                        file.write_all(&buffer)?;
                        self.stream_backup = Some(file);
                    }
                }
                in_disk_data = true;
                buffer.clear();
            }
            encode_frame(&mut buffer, row.key(), row_bytes);
        }
        if in_disk_data {
            let Some(mut file) = self.stream_backup.take() else {
                unreachable!()
            };
            file.write_all(&buffer)?;
            buffer.clear();
            let source = StreamSource::Disk {
                f: file,
                buffer,
                file_offset: 0,
                buffer_offset: 0,
                page_size: MEM_CAP.max(max_frame),
                nrows,
                rrows: 0,
            };
            self.stream_source = source;
        } else {
            let source = StreamSource::Mem {
                buffer,
                offset: 0,
                nrows: nrows as _,
            };
            self.stream_source = source;
        }
        Ok(())
    }
}
