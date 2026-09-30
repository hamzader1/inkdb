use std::{
    fs::{File, OpenOptions},
    io::Write,
};

use crate::{
    InkResult, MemCursor,
    backend::{
        executor::{Row, context::ExecCtx, sort::SortSource},
        planner::plan::Plan,
    },
    record::Record,
    varint::encode_varint,
    vfs::Vfs,
};

use super::StreamSource;

const MEM_CAP: usize = 4096 * 5;
const FILE: &str = "ink_update";

#[derive(Debug)]
pub struct MaterializedResult<V: Vfs> {
    child: Box<Plan<V>>,
    stream_source: StreamSource,
    stream_backup: Option<File>,
    nread: usize,
}

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
impl<V: Vfs> MaterializedResult<V> {
    pub fn new(child: Box<Plan<V>>) -> Self {
        Self {
            child,
            stream_source: StreamSource::None,
            stream_backup: None,
            nread: 0,
        }
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if let StreamSource::None = self.stream_source {
            self.collect(ctx)?;
        }
        self.stream_source.yield_from_stream(&mut self.nread)
    }
    fn collect(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let mut buffer = Vec::new();
        let mut varint_buffer = [0u8; 9];
        let f = |file: &mut File, buffer: &[u8]| file.write_all(buffer);
        let mut in_disk_data = false;
        let mut nrows = 0;
        while let Some(row) = self.child.next(ctx)? {
            nrows += 1;
            let row_bytes = row.stored_bytes().unwrap();
            if row_bytes.len() + buffer.len() > MEM_CAP {
                match self.stream_backup {
                    Some(ref mut file) => {
                        f(file, &buffer)?;
                    }
                    None => {
                        let mut file = V::open_temp_file(V::create_temp_file(FILE))?;
                        f(&mut file, &buffer)?;
                        self.stream_backup = Some(file);
                    }
                }
                in_disk_data = true;
                buffer.clear();
            }
            let len = encode_varint(&mut varint_buffer, row_bytes.len() as _);
            buffer.extend_from_slice(&varint_buffer[..len]);
            buffer.extend_from_slice(row_bytes);
            // if in_disk_data
            // {
            //     self.
            // }
        }
        if !buffer.is_empty() && in_disk_data {
            let Some(ref mut file) = self.stream_backup else {
                unreachable!()
            };
            file.write_all(&buffer)?;
        }
        if in_disk_data {
            let Some(file) = self.stream_backup.take() else {
                unreachable!()
            };
            let source = StreamSource::Disk {
                f: file,
                buffer,
                file_offset: 0,   /*Unused*/
                buffer_offset: 0, /*Unused*/
                page_size: 0,     /*Unused*/
                nrows,
                rrows: 0, /*Unused*/
            };
            self.stream_source = source;
        } else {
            dbg!(buffer.len(), MEM_CAP);
            // panic!();
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
