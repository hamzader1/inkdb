#[cfg(feature = "debug-instrument")]
pub mod breakpoint;
#[cfg(feature = "debug-instrument")]
pub mod replay_vfs;
#[cfg(feature = "debug-instrument")]
pub mod seq;
#[cfg(feature = "debug-instrument")]
pub mod trace;

#[cfg(feature = "debug-instrument")]
pub use breakpoint::*;
#[cfg(feature = "debug-instrument")]
pub use replay_vfs::*;
#[cfg(feature = "debug-instrument")]
pub use seq::*;
#[cfg(feature = "debug-instrument")]
pub use trace::*;

#[cfg(not(feature = "debug-instrument"))]
pub mod seq {
    #[inline(always)]
    pub fn next_seq() -> u64 {
        0
    }
    #[inline(always)]
    pub fn current_seq() -> u64 {
        0
    }
    #[inline(always)]
    pub fn set_seq(_: u64) {}
    #[inline(always)]
    pub fn push_ctx(_: u64) {}
    #[inline(always)]
    pub fn pop_ctx() {}
    #[inline(always)]
    pub fn current_ctx() -> Option<u64> {
        None
    }
}

#[cfg(not(feature = "debug-instrument"))]
pub mod trace {
    #[derive(Default, Clone, Copy)]
    pub struct TraceEvent;
    pub enum EventKind {}
    #[derive(Default, Clone, Copy)]
    pub struct EventData;
    impl EventData {
        #[inline(always)]
        pub fn insert(_: i64, _: u32, _: u32, _: u16, _: bool, _: bool) -> Self {
            EventData
        }
        #[inline(always)]
        pub fn page(_: u32, _: u8, _: u16, _: u16, _: bool) -> Self {
            EventData
        }
        #[inline(always)]
        pub fn split(_: u32, _: u32, _: u32, _: i64) -> Self {
            EventData
        }
        #[inline(always)]
        pub fn cursor(_: u8, _: u8, _: u32, _: u16) -> Self {
            EventData
        }
    }
    #[inline(always)]
    pub fn is_enabled() -> bool {
        false
    }
    #[inline(always)]
    pub fn with<F, R>(_f: F) -> R
    where
        F: FnOnce() -> R,
    {
        panic!("trace not enabled")
    }
    #[inline(always)]
    pub fn push_event(_: TraceEvent) {}
    #[inline(always)]
    pub fn get_range(_: u64, _: u64) -> Vec<TraceEvent> {
        Vec::new()
    }
    #[inline(always)]
    pub fn find_event(_: u64) -> Option<TraceEvent> {
        None
    }
    #[inline(always)]
    pub fn export_json(_: &std::path::Path) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(not(feature = "debug-instrument"))]
pub mod breakpoint {
    use crate::debug::trace::TraceEvent;
    pub fn set_break_at_seq(_: u64) {}
    pub fn set_break_at_kind(_: u8) {}
    pub fn add_break_condition<F>(_: F)
    where
        F: Fn(&TraceEvent) -> bool + 'static,
    {
    }
    pub fn set_auto_dump(_: bool) {}
    pub fn clear_breakpoints() {}
    pub fn check_breakpoint(_: &TraceEvent) {}
    pub fn get_break_hit_count() -> u64 {
        0
    }
    pub fn set_break_on_row_id(_: i64) {}
    pub fn set_break_on_page(_: u32) {}
}

#[cfg(not(feature = "debug-instrument"))]
pub mod replay_vfs {
    pub fn create_recorder(
        inner: Box<dyn crate::vfs::file::SqliteFile>,
    ) -> Box<dyn crate::vfs::file::SqliteFile> {
        inner
    }
    pub fn create_player(
        inner: Box<dyn crate::vfs::file::SqliteFile>,
        _: &std::path::Path,
    ) -> std::io::Result<Box<dyn crate::vfs::file::SqliteFile>> {
        Ok(inner)
    }
}
