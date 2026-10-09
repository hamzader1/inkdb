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
/// The name of the temporary file the collected rows spill into. It is reused
/// between runs rather than recreated, so the name is fixed.
const FILE: &str = "ink_update_temp";

/// Runs its child to completion, collects every row, then yields them one by one.
///
/// This plan was created mainly for the UPDATE plan, which deletes and reinserts
/// rows while the underlying operator is still scanning for rows that match the
/// WHERE clause. For example, deleting row A and inserting it again could cause
/// the scan to visit the same row repeatedly, potentially leading to an infinite
/// loop.
///
/// To prevent this, we collect all rows before yielding any of them. Rows stay
/// in memory while they fit within the memory limit and spill to a temporary
/// file when that limit is exceeded. This allows us to safely yield rows from
/// a StreamSource without the ongoing scan being affected by changes to the
/// underlying data.
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
    /// The operator whose rows are collected.
    pub fn child(&self) -> &Plan<V> {
        &self.child
    }
    /// Hand back one collected row.
    ///
    /// The first call collects everything the child has, and the rest read it
    /// back. The temporary file, if one was used, goes away when the last row has
    /// been handed out.
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
    /// Pull every row from the child.
    ///
    /// Rows are kept as frames in one growing buffer until the buffer fills, at
    /// which point it is written to a temporary file and started again. Which of
    /// the two the rows ended up in decides where they are read back from.
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
