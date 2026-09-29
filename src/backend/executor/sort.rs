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
    fn yield_row(&mut self) -> InkResult<Option<Row>> {
        match self.sort_source {
            SortSource::Mem {
                ref mut buffer,
                ref mut offset,
                nrows,
            } => {
                if self.nread == nrows {
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
            _ => panic!("Disk not implemented yet"),
        }
    }
    fn sort(&mut self, ctx: &mut ExecCtx<'_, V>) -> InkResult<()> {
        let mut unsorted_buffer: Vec<u8> = Vec::new();
        let mut sorted_buffer: Vec<u8> = Vec::new();
        let mut data_buffer: Vec<InnerSortBuffer> = Vec::new();
        let mut offset = 0;
        let mut n_of_runs = 1;
        // let mut n_of_rows = 0u32;
        while let Some(row) = self.child.next(ctx)? {
            let mut vint_buffer = [0u8; 9];
            let row_bytes = row.stored_bytes().unwrap();
            let record = Record::new(row_bytes)?;
            let key = Eval::eval(ctx.arena, self.index, Some(&record))?;
            let len_varint = encode_varint(&mut vint_buffer, row_bytes.len() as _);
            /*We need to create a new file*/
            if len_varint + row_bytes.len() + unsorted_buffer.len() > MEM_CAP {
                self.sort_buffer(&unsorted_buffer, &mut sorted_buffer, &mut data_buffer);
                let file = new_file(&format!("run_{}", n_of_runs))?;
                unsorted_buffer.extend_from_slice(&u32::to_be_bytes(data_buffer.len() as _)); /*Last four bytes holds the number of rows*/
                file.write_all_at(&sorted_buffer, 0)?;
                unsorted_buffer.clear();
                data_buffer.clear();
                offset = 0;
                n_of_runs += 1;
            }
            let key_data =
                InnerSortBuffer::new(key.to_owned_static(), offset, len_varint + row_bytes.len());
            // dbg!(&key_data, len_varint, len_varint + row_bytes.len());
            data_buffer.push(key_data);
            unsorted_buffer.extend_from_slice(&vint_buffer[..len_varint]);
            unsorted_buffer.extend_from_slice(row_bytes);
            offset += len_varint + row_bytes.len();
            // data_buffer.push(
            // n_of_rows += 1;
        }

        if n_of_runs == 1 {
            self.sort_buffer(&unsorted_buffer, &mut sorted_buffer, &mut data_buffer);
            self.sort_source = SortSource::Mem {
                buffer: sorted_buffer,
                offset: 0,
                nrows: data_buffer.len(),
            };
            self.is_sorted = true;
        } else {
            panic!("sort needs disk")
        }
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
