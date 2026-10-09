use std::collections::BinaryHeap;

use super::super::planner::plan;
use super::MEM_CAP;
use crate::backend::executor::Row;
use crate::backend::executor::context::ExecCtx;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::{RowView, StreamSource, decode_frame};
use crate::backend::planner::plan::Plan;
use crate::record::{Record, Value};
use crate::varint::encode_varint;
use crate::vfs::Vfs;
use crate::vfs::file::InkFile;
use crate::{InkResult, MemCursor};

/// A name for this sort's temporary files, made unique across the process so
/// that two sorts running at once do not write over each other.
fn temp_prefix() -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ink_sort_{}_{}", std::process::id(), id)
}

/// Orders the rows of its child by one expression.
///
/// The rows are pulled in, each with the value it sorts by worked out, and sorted
/// in memory while they fit. Once they no longer fit, what has been sorted so far
/// is written to a run on disk and the sort starts again with an empty buffer;
/// at the end the runs are merged. Rows that end up on disk are read back the
/// same way a materialized result is.
#[derive(Debug)]
pub struct Sort<V: Vfs> {
    child: Box<plan::Plan<V>>,
    sort_source: StreamSource<V>,
    desc: bool,   /*Asc is the default*/
    index: usize, /*Arena index*/
    is_sorted: bool,
    nread: usize, /*Number of reads*/
    is_done: bool,
    max_frame: usize,
    temp: String,
    rowid_column: Option<usize>,
    rowid_captured: bool,
}

impl<V: Vfs> Sort<V> {
    pub fn new(child: Box<plan::Plan<V>>, index: usize, desc: bool) -> Self {
        Self {
            child,
            index,
            desc,
            sort_source: StreamSource::None,
            nread: 0,
            is_sorted: false,
            is_done: false,
            max_frame: 0,
            temp: temp_prefix(),
            rowid_column: None,
            rowid_captured: false,
        }
    }
    /// The operator whose rows are sorted.
    pub fn child(&self) -> &Plan<V> {
        self.child.as_ref()
    }
    /// Hand back one row in order.
    ///
    /// The first call sorts everything; the rest read the sorted rows back. The
    /// temporary files go away once the last row has been handed out.
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.is_done {
            return Ok(None);
        }
        if !self.is_sorted {
            self.sort(ctx)?;
        }
        let row = self
            .sort_source
            .yield_from_stream(&mut self.nread, self.rowid_column)?;
        if row.is_none() {
            self.is_done = true;
            let _ = ctx.pager.vfs_mut().remove_temp(self.out_path());
        }
        Ok(row)
    }
    /// The arena index of the expression the sort is by.
    pub fn id(&self) -> usize {
        self.index
    }
    /// The file holding one sorted run.
    fn run_path(&self, run: usize) -> String {
        format!("{}_run_{}", self.temp, run)
    }
    /// The file the merged output is written to.
    fn out_path(&self) -> String {
        format!("{}_out", self.temp)
    }
    /// Pull every row, sorting as it goes.
    ///
    /// Each row is written into a buffer as a frame with the sort key in front of
    /// it, and the keys are kept alongside the frames so the frames can be
    /// reordered without being decoded. When the buffer would overflow, the part
    /// gathered so far is sorted and written out as one run. If only one run was
    /// ever needed the whole result is in memory, and otherwise the runs are
    /// merged.
    fn sort(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let mut unsorted_buffer: Vec<u8> = Vec::new();
        let mut sorted_buffer: Vec<u8> = Vec::new();
        let mut data_buffer: Vec<InnerSortBuffer> = Vec::new();
        let mut offset = 0;
        let mut n_of_runs = 1;
        while let Some(row) = self.child.next(ctx)? {
            // dbg!(row.to_values());
            // dbg!(row.value(1));
            // dbg!(row.len());
            if !self.rowid_captured {
                self.rowid_column = row.rowid_column();
                self.rowid_captured = true;
            }
            let mut key_buffer = [0u8; 9];
            let mut len_buffer = [0u8; 9];
            let row_bytes = row.stored_bytes().unwrap();
            let view = RowView::new(row.key(), Record::new(row_bytes)?, row.rowid_column());
            debug_assert!(
                ctx.arena.nodes.get(self.index).is_some(),
                "sort key must be a bound arena node, got {} of {}",
                self.index,
                ctx.arena.nodes.len()
            );
            let key = Eval::eval(ctx.arena, self.index, Some(&view))?;
            let key_bytes = encode_varint(&mut key_buffer, row.key());
            let payload = key_bytes + row_bytes.len();
            let len_varint = encode_varint(&mut len_buffer, payload as u64);
            let frame_len = len_varint + payload;
            self.max_frame = self.max_frame.max(frame_len);
            /*We need to create a new file*/
            if frame_len + unsorted_buffer.len() > MEM_CAP {
                self.sort_buffer(
                    &unsorted_buffer,
                    &mut sorted_buffer,
                    &mut data_buffer,
                    self.desc,
                );
                let mut file = ctx.pager.vfs_mut().open_temp(self.run_path(n_of_runs))?;
                sorted_buffer.extend_from_slice(&u32::to_be_bytes(data_buffer.len() as _)); /*Last four bytes holds the number of rows*/
                file.set_len(0)?;
                file.write_all(&sorted_buffer)?;
                unsorted_buffer.clear();
                data_buffer.clear();
                offset = 0;
                n_of_runs += 1;
            }
            let key_data = InnerSortBuffer::new(key.to_owned_static(), offset, frame_len);
            data_buffer.push(key_data);
            unsorted_buffer.extend_from_slice(&len_buffer[..len_varint]);
            unsorted_buffer.extend_from_slice(&key_buffer[..key_bytes]);
            unsorted_buffer.extend_from_slice(row_bytes);
            offset += frame_len;
        }

        if n_of_runs == 1 {
            self.sort_buffer(
                &unsorted_buffer,
                &mut sorted_buffer,
                &mut data_buffer,
                self.desc,
            );
            self.sort_source = StreamSource::Mem {
                buffer: sorted_buffer,
                offset: 0,
                nrows: data_buffer.len(),
            };
        } else {
            /*We flush the last one*/
            self.sort_buffer(
                &unsorted_buffer,
                &mut sorted_buffer,
                &mut data_buffer,
                self.desc,
            );
            let mut file = ctx.pager.vfs_mut().open_temp(self.run_path(n_of_runs))?;
            sorted_buffer.extend_from_slice(&u32::to_be_bytes(data_buffer.len() as _)); /*Last four bytes holds the number of rows*/
            file.set_len(0)?;
            file.write_all(&sorted_buffer)?;
            self.external_sort(n_of_runs, ctx)?;
        }
        self.is_sorted = true;
        Ok(())
    }
    /// Merge the sorted runs into one stream.
    ///
    /// Every run is read a page at a time, and the first row of each goes into a
    /// heap ordered by the sort key. The smallest is taken out, written to the
    /// output, and the next row of that same run is put in its place, which walks
    /// all the runs together in order. A descending sort is handled by reversing
    /// each buffer as it is written, so the heap only ever has to compare one way.
    fn external_sort(&mut self, nruns: usize, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let page_size = (MEM_CAP / (nruns + 1)).max(self.max_frame).max(64);
        let mut sort_buffers = Vec::new();
        let mut page_buffer: Vec<u8> = Vec::with_capacity(page_size as _);
        for i in 0..nruns {
            let run_id = i + 1;
            let mut file = ctx.pager.vfs_mut().open_temp(self.run_path(run_id))?;
            let file_len = file.len()?;
            let mut nrows_buffer = [0u8; 4];
            file.read_exact_at(file_len - 4, &mut nrows_buffer)?;
            let nrows = u32::from_be_bytes(nrows_buffer);
            let mut children = Vec::new();
            let mut offset = 0;
            let rrows = load_page(
                &mut file,
                &mut page_buffer,
                nrows as _,
                page_size,
                &mut offset,
            )?;
            load_children(
                &page_buffer,
                &mut children,
                rrows,
                self.index,
                self.rowid_column,
                ctx,
            )?;
            let sort_buffer = SortBuffer::new(
                file,
                run_id,
                rrows,
                nrows as _,
                std::mem::take(&mut page_buffer),
                children,
                offset as _,
            );
            sort_buffers.push(sort_buffer);
            // temp_buffer.clear();
        }
        let mut heap = BinaryHeap::with_capacity(nruns);
        /*O(k) where k: nruns*/
        for (i, sort_buffer) in sort_buffers.iter().enumerate() {
            let Some(first) = sort_buffer.children.first() else {
                continue;
            };
            let entry = HeapEntry::new(first.key.clone(), i, 0);
            heap.push(entry);
        }
        let mut output_buffer = Vec::with_capacity(page_size);
        let mut output_file = ctx.pager.vfs_mut().open_temp(self.out_path())?;
        output_file.set_len(0)?;
        let mut writte_nrows = 0u32;
        let mut sorted_buffer = Vec::new();
        let mut data = Vec::new();
        loop {
            let Some(entry) = heap.pop() else {
                self.sort_buffer(&output_buffer, &mut sorted_buffer, &mut data, self.desc);
                output_file.write_all(&sorted_buffer)?;
                break;
            };
            let child = &sort_buffers[entry.buffer_id].children[entry.child_id];
            if child.len + output_buffer.len() > page_size {
                self.sort_buffer(&output_buffer, &mut sorted_buffer, &mut data, self.desc);
                output_file.write_all(&sorted_buffer)?;
                output_buffer.clear();
                sorted_buffer.clear();
                data.clear();
            }
            let start = output_buffer.len();
            let data_entry = InnerSortBuffer::new(entry.key, start, child.len);
            data.push(data_entry);
            output_buffer.extend_from_slice(
                &sort_buffers[entry.buffer_id].page_buffer[child.start..child.start + child.len],
            );

            match sort_buffers[entry.buffer_id].yield_entry(
                entry.child_id + 1,
                page_size,
                self.index,
                self.rowid_column,
                ctx,
            )? {
                Some(e) => {
                    heap.push(e);
                }
                None => {
                    let target_buffer = &sort_buffers[entry.buffer_id];
                    assert!(target_buffer.nrows == target_buffer.rrows);
                    writte_nrows += sort_buffers[entry.buffer_id].nrows as u32;
                }
            }
        }
        let mut buffer: Vec<u8> = std::mem::take(&mut output_buffer);
        let mut file_offset = 0;
        let rrows = load_page(
            &mut output_file,
            &mut buffer,
            writte_nrows as _,
            page_size,
            &mut file_offset,
        )?;
        let sort_source = StreamSource::Disk {
            f: output_file,
            buffer,
            file_offset,
            buffer_offset: 0,
            page_size,
            nrows: writte_nrows,
            rrows,
        };
        for run in 1..=nruns {
            /*
             * Do not Try
             */
            let _ = ctx.pager.vfs_mut().remove_temp(self.run_path(run));
        }
        self.sort_source = sort_source;
        Ok(())
    }
    /// Sort the frames gathered so far and lay them out in order.
    ///
    /// The keys are sorted rather than the frames, and descending is the same
    /// sort read backwards, so the frames are copied out in whichever order the
    /// sort left the keys.
    fn sort_buffer(
        &mut self,
        inp: &[u8],
        out: &mut Vec<u8>,
        data: &mut [InnerSortBuffer],
        desc: bool,
    ) {
        out.clear();
        data.sort_by(|a, b| a.key.cmp(&b.key));
        if desc {
            data.reverse();
        }
        for inner in data.iter() {
            out.extend_from_slice(&inp[inner.start..inner.start + inner.len]);
        }
    }
}

/// Read whole frames out of a spilled file into a buffer.
///
/// Frames are read until the row limit or the buffer size is reached, so the
/// buffer always holds whole frames and ends on a boundary. The answer is how
/// many were read, and the offset is moved past them.
pub fn load_page<F: InkFile>(
    file: &mut F,
    out: &mut Vec<u8>,
    limit: usize,
    chunk_size: usize,
    offset: &mut usize,
) -> InkResult<usize> {
    out.clear();
    let remaining = file.len()?.saturating_sub(*offset as u64);
    let read_len = remaining.min(chunk_size as u64) as usize;
    let mut temp_buffer = vec![0u8; read_len];
    file.read_exact_at(*offset as u64, &mut temp_buffer)?;

    let mut pos = 0usize;
    let mut niter = 0usize;
    for _ in 0..limit {
        let available = temp_buffer.len() - pos;
        if available == 0 {
            break;
        }
        let mut cursor = MemCursor::new(&temp_buffer[pos..]);
        let Ok((row_len, consumed)) = cursor.read_next_varint(available) else {
            break;
        };
        let frame_len = consumed + row_len as usize;
        if frame_len > available || frame_len + out.len() > chunk_size {
            break;
        }
        out.extend_from_slice(&temp_buffer[pos..pos + frame_len]);
        pos += frame_len;
        *offset += frame_len;
        niter += 1; /*nIter: number of iterations*/
    }
    Ok(niter)
}

/// Work out the sort key of every frame in a buffer, so they can be ordered.
fn load_children<V: Vfs>(
    inp: &[u8],
    children: &mut Vec<InnerSortBuffer>,
    limit: usize,
    arena_index: usize,
    rowid_column: Option<usize>,
    ctx: &mut ExecCtx<'_, V>,
) -> InkResult<()> {
    children.clear();
    let mut pos = 0usize;
    for _ in 0..limit {
        let available = inp.len() - pos;
        if available == 0 {
            break;
        }
        let mut cursor = MemCursor::new(&inp[pos..]);
        let Ok((len, consumed)) = cursor.read_next_varint(available) else {
            break;
        };
        let frame_len = consumed + len as usize;
        if frame_len > available {
            break;
        }
        let (row_key, record_bytes) = decode_frame(&inp[pos..pos + frame_len])?;
        let view = RowView::new(row_key, Record::new(record_bytes)?, rowid_column);
        let key = Eval::eval(ctx.arena, arena_index, Some(&view))?.into_static();
        children.push(InnerSortBuffer::new(key, pos, frame_len));
        pos += frame_len;
    }
    Ok(())
}

/// One run on disk, with the page of it that is currently in memory.
struct SortBuffer<V: Vfs> {
    f: V::File,
    rid: usize,   /*Run id*/
    rrows: usize, /*read rows*/
    nrows: usize, /*number of rows*/
    page_buffer: Vec<u8>,
    children: Vec<InnerSortBuffer>,
    offset: usize,
}

impl<V: Vfs> SortBuffer<V> {
    #[allow(clippy::too_many_arguments)] /*Temporary*/
    fn new(
        f: V::File,
        rid: usize,
        rrows: usize,
        nrows: usize,
        page_buffer: Vec<u8>,
        children: Vec<InnerSortBuffer>,
        offset: usize,
    ) -> Self {
        Self {
            f,
            rid,
            rrows,
            nrows,
            page_buffer,
            children,
            offset,
        }
    }
    /// The heap entry for one row of this run.
    ///
    /// When the page has been walked to its end, the next page is read in and the
    /// walk starts again. Nothing is returned once the run has no rows left.
    fn yield_entry(
        &mut self,
        mut child_id: usize,
        page_size: usize,
        index: usize,
        rowid_column: Option<usize>,
        ctx: &mut ExecCtx<'_, V>,
    ) -> InkResult<Option<HeapEntry>> {
        if child_id == self.children.len() {
            if self.rrows == self.nrows {
                return Ok(None);
            }
            let niter = load_page(
                &mut self.f,
                &mut self.page_buffer,
                self.nrows - self.rrows,
                page_size,
                &mut self.offset,
            )?;
            if niter == 0 {
                return Ok(None);
            }
            self.rrows += niter;
            load_children(
                &self.page_buffer,
                &mut self.children,
                niter,
                index,
                rowid_column,
                ctx,
            )?;
            child_id = 0;
        }
        if child_id >= self.children.len() {
            return Ok(None);
        }
        let entry = HeapEntry::new(self.children[child_id].key.clone(), self.rid - 1, child_id);
        Ok(Some(entry))
    }
}

/// Where one frame sits in the buffer it is held in, with the value it sorts by.
#[derive(Debug, Clone)]
struct InnerSortBuffer {
    key: Value<'static>,
    start: usize,
    len: usize,
}
impl InnerSortBuffer {
    fn new(key: Value<'static>, start: usize, len: usize) -> Self {
        Self { key, start, len }
    }
}

/// One run's next row, ordered by its sort key.
///
/// The comparison is reversed so that a `BinaryHeap`, which is a max heap, hands
/// back the smallest key first.
#[derive(Debug)]
struct HeapEntry {
    key: Value<'static>,
    buffer_id: usize,
    child_id: usize,
}

impl HeapEntry {
    fn new(key: Value<'static>, buffer_id: usize, child_id: usize) -> Self {
        Self {
            key,
            buffer_id,
            child_id,
        }
    }
}
impl Ord for HeapEntry {
    /// Reversed, so the smallest key comes out first.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.key.cmp(&self.key)
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for HeapEntry {}
