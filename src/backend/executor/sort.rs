use core::panic;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;

use super::super::planner::plan;
use crate::backend::executor::context::ExecCtx;
use crate::backend::executor::eval::Eval;
use crate::backend::executor::{Row, RowView};
use crate::backend::planner::plan::Plan;
use crate::record::{Record, Value};
use crate::varint::encode_varint;
use crate::vfs::Vfs;
use crate::{InkResult, MemCursor};

const MEM_CAP: usize = 10 * 1024 * 1024; /*10 MiB*/

fn temp_prefix() -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("ink_sort_{}_{}", std::process::id(), id)
}

#[derive(Debug)]
pub struct Sort<V: Vfs> {
    child: Box<plan::Plan<V>>,
    sort_source: SortSource,
    index: usize, /*Arena index*/
    is_sorted: bool,
    nread: usize,
    is_done: bool,
    max_frame: usize,
    temp: String,
}

impl<V: Vfs> Sort<V> {
    pub fn new(child: Box<plan::Plan<V>>, index: usize) -> Self {
        Self {
            child,
            index,
            sort_source: SortSource::None,
            nread: 0,
            is_sorted: false,
            is_done: false,
            max_frame: 0,
            temp: temp_prefix(),
        }
    }
    pub fn child(&self) -> &Plan<V> {
        self.child.as_ref()
    }
    pub fn next(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<Option<Row>> {
        if self.is_done {
            return Ok(None);
        }
        if !self.is_sorted {
            self.sort(ctx)?;
        }
        self.yield_row()
    }
    pub fn id(&self) -> usize {
        self.index
    }
    fn run_path(&self, run: usize) -> String {
        std::env::temp_dir()
            .join(format!("{}_run_{}", self.temp, run))
            .to_string_lossy()
            .into_owned()
    }
    fn out_path(&self) -> String {
        std::env::temp_dir()
            .join(format!("{}_out", self.temp))
            .to_string_lossy()
            .into_owned()
    }
    fn yield_row(&mut self) -> InkResult<Option<Row>> {
        match self.sort_source {
            SortSource::Mem {
                ref mut buffer,
                ref mut offset,
                nrows,
            } => {
                if self.nread == nrows {
                    self.is_done = true;
                    return Ok(None);
                }
                let buffer = &buffer[*offset..];
                let mut cursor = MemCursor::new(buffer);
                let (len, consumed) = cursor.read_next_varint(buffer.len())?;
                let row = Row::stored(0, buffer[consumed..consumed + len as usize].into());
                self.nread += 1;
                *offset += consumed + len as usize;

                Ok(Some(row))
            }
            SortSource::Disk {
                ref mut f,
                ref mut buffer,
                ref mut buffer_offset,
                ref mut file_offset,
                nrows,
                ref mut rrows,
                page_size,
            } => {
                if self.nread == nrows as usize {
                    self.is_done = true;
                    return Ok(None);
                }
                if *buffer_offset == buffer.len() {
                    let remaining = nrows as usize - *rrows;
                    if remaining == 0 {
                        self.is_done = true;
                        return Ok(None);
                    }
                    let r = load_page(f, buffer, remaining, page_size, file_offset)?;
                    if r == 0 {
                        self.is_done = true;
                        return Ok(None);
                    }
                    *rrows += r;
                    *buffer_offset = 0;
                }
                let buffer = &buffer[*buffer_offset..];
                let mut cursor = MemCursor::new(buffer);
                let (len, consumed) = cursor.read_next_varint(buffer.len())?;
                let row = Row::stored(0, buffer[consumed..consumed + len as usize].to_vec());
                *buffer_offset += consumed + len as usize;
                self.nread += 1;
                Ok(Some(row))
            }
            SortSource::None => Ok(None),
        }
    }
    fn sort(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let mut unsorted_buffer: Vec<u8> = Vec::new();
        let mut sorted_buffer: Vec<u8> = Vec::new();
        let mut data_buffer: Vec<InnerSortBuffer> = Vec::new();
        let mut offset = 0;
        let mut n_of_runs = 1;
        while let Some(row) = self.child.next(ctx)? {
            let mut vint_buffer = [0u8; 9];
            let row_bytes = row.stored_bytes().unwrap();
            let record = Record::new(row_bytes)?;
            let key = Eval::eval(ctx.arena, self.index, Some(&record))?;
            let len_varint = encode_varint(&mut vint_buffer, row_bytes.len() as _);
            self.max_frame = self.max_frame.max(len_varint + row_bytes.len());
            /*We need to create a new file*/
            if len_varint + row_bytes.len() + unsorted_buffer.len() > MEM_CAP {
                self.sort_buffer(&unsorted_buffer, &mut sorted_buffer, &mut data_buffer);
                let mut file = new_file(&self.run_path(n_of_runs))?;
                sorted_buffer.extend_from_slice(&u32::to_be_bytes(data_buffer.len() as _)); /*Last four bytes holds the number of rows*/
                file.write_all_at(&sorted_buffer, 0)?;
                file.flush()?;
                unsorted_buffer.clear();
                data_buffer.clear();
                offset = 0;
                n_of_runs += 1;
            }
            let key_data =
                InnerSortBuffer::new(key.to_owned_static(), offset, len_varint + row_bytes.len());
            data_buffer.push(key_data);
            unsorted_buffer.extend_from_slice(&vint_buffer[..len_varint]);
            unsorted_buffer.extend_from_slice(row_bytes);
            offset += len_varint + row_bytes.len();
        }

        if n_of_runs == 1 {
            self.sort_buffer(&unsorted_buffer, &mut sorted_buffer, &mut data_buffer);
            self.sort_source = SortSource::Mem {
                buffer: sorted_buffer,
                offset: 0,
                nrows: data_buffer.len(),
            };
        } else {
            /*We flush the last one*/
            self.sort_buffer(&unsorted_buffer, &mut sorted_buffer, &mut data_buffer);
            let mut file = new_file(&self.run_path(n_of_runs))?;
            sorted_buffer.extend_from_slice(&u32::to_be_bytes(data_buffer.len() as _)); /*Last four bytes holds the number of rows*/
            file.write_all_at(&sorted_buffer, 0)?;
            file.flush()?;
            self.external_sort(n_of_runs, ctx)?;
        }
        self.is_sorted = true;
        Ok(())
    }
    fn external_sort(&mut self, nruns: usize, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let page_size = (MEM_CAP / (nruns + 1)).max(self.max_frame).max(64);
        let mut sort_buffers = Vec::new();
        let mut temp_buffer = vec![0u8; page_size];
        let mut page_buffer: Vec<u8> = Vec::with_capacity(page_size as _);
        for i in 0..nruns {
            let run_id = i + 1;
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(self.run_path(run_id))?;
            let file_len = file.metadata()?.len();
            let mut nrows_buffer = [0u8; 4];
            file.read_exact_at(&mut nrows_buffer, file_len - 4);
            let nrows = u32::from_be_bytes(nrows_buffer);
            file.read_exact_at(&mut temp_buffer, 0)?;
            let mut children = Vec::new();
            let mut cursor = MemCursor::new(&temp_buffer);
            let mut rrows = 0;
            let mut offset = 0;
            rrows = load_page(
                &mut file,
                &mut page_buffer,
                nrows as _,
                page_size,
                &mut offset,
            )?;
            load_children(&page_buffer, &mut children, rrows, self.index, ctx)?;
            let sort_buffer = SortBuffer::new(
                file,
                run_id,
                rrows,
                nrows as _,
                std::mem::take(&mut page_buffer),
                children,
                offset as _,
                false,
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
        let mut output_file = new_file(&self.out_path())?;
        let mut write_offset = 0;
        let mut writte_nrows = 0u32;
        let mut sorted_buffer = Vec::new();
        let mut data = Vec::new();
        loop {
            let Some(entry) = heap.pop() else {
                self.sort_buffer(&output_buffer, &mut sorted_buffer, &mut data);
                output_file.write_all(&sorted_buffer)?;
                output_file.flush()?;
                break;
            };
            let child = &sort_buffers[entry.buffer_id].children[entry.child_id];
            if child.len + output_buffer.len() > page_size {
                self.sort_buffer(&output_buffer, &mut sorted_buffer, &mut data);
                output_file.write_all(&sorted_buffer)?;
                output_file.flush()?;
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
        let sort_source = SortSource::Disk {
            f: output_file,
            buffer,
            file_offset,
            buffer_offset: 0,
            page_size,
            nrows: writte_nrows,
            rrows,
        };
        for run in 1..=nruns {
            let _ = std::fs::remove_file(self.run_path(run));
        }
        self.sort_source = sort_source;
        Ok(())
    }
    fn sort_buffer(&mut self, inp: &[u8], out: &mut Vec<u8>, data: &mut [InnerSortBuffer]) {
        out.clear();
        data.sort_by(|a, b| a.key.cmp(&b.key));
        for inner in data.iter() {
            out.extend_from_slice(&inp[inner.start..inner.start + inner.len]);
        }
    }
}

fn new_file(file_name: &str) -> InkResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(file_name)?;
    Ok(file)
}
struct SortBuffer {
    f: File,      /*Change to Vfs*/
    rid: usize,   /*Run id*/
    rrows: usize, /*read rows*/
    nrows: usize, /*number of rows*/
    page_buffer: Vec<u8>,
    children: Vec<InnerSortBuffer>,
    is_done: bool,
}
#[derive(Debug, Clone)]
struct InnerSortBuffer {
    key: Value<'static>, /*Can we change it to 'any ?*/
    start: usize,
    len: usize,
}
impl InnerSortBuffer {
    fn new(key: Value<'static>, start: usize, len: usize) -> Self {
        Self { key, start, len }
    }
}

#[derive(Debug)]
enum SortSource {
    Mem {
        buffer: Vec<u8>,
        offset: usize,
        nrows: usize,
    },
    Disk {
        f: File,
        buffer: Vec<u8>,
        file_offset: usize,
        buffer_offset: usize,
        page_size: usize,
        nrows: u32,
        rrows: usize,
    },
    None,
}

impl<V: Vfs> Drop for Sort<V> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.out_path());
    }
}

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
