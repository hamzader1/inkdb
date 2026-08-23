#[cfg(feature = "debug-instrument")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "debug-instrument")]
use std::cell::RefCell;
#[cfg(feature = "debug-instrument")]
use std::time::Instant;

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TraceEvent {
    pub seq: u64,
    pub parent_seq: u64,
    pub kind: EventKind,
    pub timestamp_ns: u64,
    pub data: EventData,
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum EventKind {
    Insert = 1,
    Delete = 2,
    Select = 3,
    Update = 4,
    PageRead = 10,
    PageWrite = 11,
    PageAlloc = 12,
    PageFree = 13,
    Split = 20,
    Merge = 21,
    Overflow = 22,
    Underflow = 23,
    Seek = 30,
    CursorNext = 31,
    CursorPrev = 32,
    Checkpoint = 40,
    Flush = 41,
    Recovery = 42,
    Custom = 255,
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EventData {
    pub insert: Option<InsertData>,
    pub page: Option<PageData>,
    pub split: Option<SplitData>,
    pub cursor: Option<CursorData>,
}

#[cfg(feature = "debug-instrument")]
impl Default for EventData {
    fn default() -> Self {
        Self {
            insert: None,
            page: None,
            split: None,
            cursor: None,
        }
    }
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InsertData {
    pub row_id: i64,
    pub table_id: u32,
    pub page_no: u32,
    pub cell_idx: u16,
    pub is_split: bool,
    pub is_overflow: bool,
    pub _pad: [u8; 2],
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PageData {
    pub page_no: u32,
    pub page_type: u8,
    pub free_space: u16,
    pub cell_count: u16,
    pub is_dirty: bool,
    pub _pad: [u8; 3],
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SplitData {
    pub left: u32,
    pub right: u32,
    pub parent: u32,
    pub boundary: i64,
}

#[cfg(feature = "debug-instrument")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CursorData {
    pub stack_depth: u8,
    pub state: u8,
    pub page_no: u32,
    pub cell_idx: u16,
    pub _pad: [u8; 2],
}

#[cfg(feature = "debug-instrument")]
impl EventData {
    #[inline(always)]
    pub fn insert(
        row_id: i64,
        table_id: u32,
        page_no: u32,
        cell_idx: u16,
        is_split: bool,
        is_overflow: bool,
    ) -> Self {
        Self {
            insert: Some(InsertData {
                row_id,
                table_id,
                page_no,
                cell_idx,
                is_split,
                is_overflow,
                _pad: [0; 2],
            }),
            page: None,
            split: None,
            cursor: None,
        }
    }

    #[inline(always)]
    pub fn page(
        page_no: u32,
        page_type: u8,
        free_space: u16,
        cell_count: u16,
        is_dirty: bool,
    ) -> Self {
        Self {
            page: Some(PageData {
                page_no,
                page_type,
                free_space,
                cell_count,
                is_dirty,
                _pad: [0; 3],
            }),
            insert: None,
            split: None,
            cursor: None,
        }
    }

    #[inline(always)]
    pub fn split(left: u32, right: u32, parent: u32, boundary: i64) -> Self {
        Self {
            split: Some(SplitData {
                left,
                right,
                parent,
                boundary,
            }),
            insert: None,
            page: None,
            cursor: None,
        }
    }

    #[inline(always)]
    pub fn cursor(stack_depth: u8, state: u8, page_no: u32, cell_idx: u16) -> Self {
        Self {
            cursor: Some(CursorData {
                stack_depth,
                state,
                page_no,
                cell_idx,
                _pad: [0; 2],
            }),
            insert: None,
            page: None,
            split: None,
        }
    }
}

#[cfg(feature = "debug-instrument")]
pub struct TraceBuffer {
    pub buf: Vec<TraceEvent>,
    pub capacity: usize,
    pub head: usize,
    pub enabled: bool,
}

#[cfg(feature = "debug-instrument")]
impl TraceBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            capacity,
            head: 0,
            enabled: true,
        }
    }

    #[inline(always)]
    pub fn push(&mut self, mut ev: TraceEvent) {
        if !self.enabled {
            return;
        }
        ev.timestamp_ns = START_TIME.with(|s| s.elapsed().as_nanos() as u64);
        if self.buf.len() < self.capacity {
            self.buf.push(ev);
        } else {
            self.buf[self.head] = ev;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    pub fn range(&self, start_seq: u64, end_seq: u64) -> Vec<TraceEvent> {
        let mut out: Vec<_> = self
            .buf
            .iter()
            .filter(|e| e.seq >= start_seq && e.seq <= end_seq)
            .copied()
            .collect();
        out.sort_by_key(|e| e.seq);
        out
    }

    pub fn find(&self, seq: u64) -> Option<TraceEvent> {
        self.buf.iter().find(|e| e.seq == seq).copied()
    }

    pub fn all(&self) -> Vec<TraceEvent> {
        let mut out = self.buf.clone();
        out.sort_by_key(|e| e.seq);
        out
    }

    pub fn export_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::fs::File;
        let file = File::create(path)?;
        serde_json::to_writer(file, &self.all())?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[cfg(feature = "debug-instrument")]
thread_local! {
    static TRACE_BUFFER: RefCell<TraceBuffer> = RefCell::new(TraceBuffer::new(1_000_000));
    static START_TIME: Instant = Instant::now();
}

#[cfg(feature = "debug-instrument")]
pub fn with<F, R>(f: F) -> R
where
    F: FnOnce(&mut TraceBuffer) -> R,
{
    TRACE_BUFFER.with(|t| f(&mut t.borrow_mut()))
}

#[cfg(feature = "debug-instrument")]
pub fn is_enabled() -> bool {
    TRACE_BUFFER.with(|t| t.borrow().enabled)
}

#[cfg(feature = "debug-instrument")]
pub fn push_event(ev: TraceEvent) {
    TRACE_BUFFER.with(|t| t.borrow_mut().push(ev));
}

#[cfg(feature = "debug-instrument")]
pub fn get_range(start: u64, end: u64) -> Vec<TraceEvent> {
    TRACE_BUFFER.with(|t| t.borrow().range(start, end))
}

#[cfg(feature = "debug-instrument")]
pub fn find_event(seq: u64) -> Option<TraceEvent> {
    TRACE_BUFFER.with(|t| t.borrow().find(seq))
}

#[cfg(feature = "debug-instrument")]
pub fn export_json(path: &std::path::Path) -> std::io::Result<()> {
    TRACE_BUFFER.with(|t| t.borrow().export_json(path))
}

#[cfg(not(feature = "debug-instrument"))]
pub struct TraceEvent;

#[cfg(not(feature = "debug-instrument"))]
pub enum EventKind {}

#[cfg(not(feature = "debug-instrument"))]
#[derive(Default, Clone, Copy)]
pub struct EventData;

#[cfg(not(feature = "debug-instrument"))]
impl EventData {
    #[inline(always)]
    pub fn insert(_: i64, _: u32, _: u32, _: u16, _: bool, _: bool) -> Self {
        Self
    }
    #[inline(always)]
    pub fn page(_: u32, _: u8, _: u16, _: u16, _: bool) -> Self {
        Self
    }
    #[inline(always)]
    pub fn split(_: u32, _: u32, _: u32, _: i64) -> Self {
        Self
    }
    #[inline(always)]
    pub fn cursor(_: u8, _: u8, _: u32, _: u16) -> Self {
        Self
    }
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn with<F, R>(_f: F) -> R
where
    F: FnOnce() -> R,
{
    panic!("trace not enabled")
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn push_event(_ev: TraceEvent) {}

#[cfg(not(feature = "debug-instrument"))]
pub fn get_range(_start: u64, _end: u64) -> Vec<TraceEvent> {
    Vec::new()
}

#[cfg(not(feature = "debug-instrument"))]
pub fn find_event(_seq: u64) -> Option<TraceEvent> {
    None
}

#[cfg(not(feature = "debug-instrument"))]
pub fn export_json(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[macro_export]
macro_rules! trace_event {
    ($kind:expr, $data:expr) => {
        #[cfg(feature = "debug-instrument")]
        {
            if $crate::debug::trace::is_enabled() {
                let seq = $crate::debug::seq::next_seq();
                let parent = $crate::debug::seq::current_ctx().unwrap_or(0);
                $crate::debug::trace::push_event($crate::debug::trace::TraceEvent {
                    seq,
                    parent_seq: parent,
                    kind: $kind,
                    timestamp_ns: 0,
                    data: $data,
                });
            }
        }
    };
    ($kind:expr, $data:expr, $ctx:expr) => {
        #[cfg(feature = "debug-instrument")]
        {
            if $crate::debug::trace::is_enabled() {
                let seq = $crate::debug::seq::next_seq();
                $crate::debug::seq::push_ctx(seq);
                trace_event!($kind, $data);
                $ctx;
                $crate::debug::seq::pop_ctx();
            }
            #[cfg(not(feature = "debug-instrument"))]
            {
                $ctx;
            }
        }
    };
}

#[macro_export]
macro_rules! trace_point {
    ($kind:expr, $data:expr) => {
        trace_event!($kind, $data);
    };
}
