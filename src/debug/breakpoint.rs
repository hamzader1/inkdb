#[cfg(feature = "debug-instrument")]
use std::cell::RefCell;

#[cfg(feature = "debug-instrument")]
static mut BREAK_AT_SEQ: u64 = 0;

#[cfg(feature = "debug-instrument")]
static mut BREAK_AT_KIND: u8 = 0;

#[cfg(feature = "debug-instrument")]
thread_local! {
    static BREAK_CONDITIONS: RefCell<Vec<Box<dyn Fn(&crate::debug::trace::TraceEvent) -> bool>>> = RefCell::new(Vec::new());
}

#[cfg(feature = "debug-instrument")]
static mut AUTO_DUMP_ON_BREAK: bool = true;

#[cfg(feature = "debug-instrument")]
static mut BREAK_HIT_COUNT: u64 = 0;

#[cfg(feature = "debug-instrument")]
pub fn set_break_at_seq(seq: u64) {
    unsafe {
        BREAK_AT_SEQ = seq;
    }
}

#[cfg(feature = "debug-instrument")]
pub fn set_break_at_kind(kind: crate::debug::trace::EventKind) {
    unsafe {
        BREAK_AT_KIND = kind as u8;
    }
}

#[cfg(feature = "debug-instrument")]
pub fn add_break_condition<F>(f: F)
where
    F: Fn(&crate::debug::trace::TraceEvent) -> bool + 'static,
{
    BREAK_CONDITIONS.with(|c| c.borrow_mut().push(Box::new(f)));
}

#[cfg(feature = "debug-instrument")]
pub fn set_auto_dump(enabled: bool) {
    unsafe {
        AUTO_DUMP_ON_BREAK = enabled;
    }
}

#[cfg(feature = "debug-instrument")]
pub fn clear_breakpoints() {
    unsafe {
        BREAK_AT_SEQ = 0;
        BREAK_AT_KIND = 0;
    }
    BREAK_CONDITIONS.with(|c| c.borrow_mut().clear());
}

#[cfg(feature = "debug-instrument")]
pub fn check_breakpoint(ev: &crate::debug::trace::TraceEvent) {
    let hit = unsafe {
        (BREAK_AT_SEQ != 0 && ev.seq == BREAK_AT_SEQ)
            || (BREAK_AT_KIND != 0 && ev.kind as u8 == BREAK_AT_KIND)
    } || BREAK_CONDITIONS.with(|c| c.borrow().iter().any(|f| f(ev)));
    if hit {
        hit_breakpoint(ev);
    }
}

#[cfg(feature = "debug-instrument")]
fn hit_breakpoint(ev: &crate::debug::trace::TraceEvent) {
    unsafe {
        BREAK_HIT_COUNT += 1;
    }
    eprintln!(
        "\nBREAK #{}: seq={} kind={:?}",
        unsafe { BREAK_HIT_COUNT },
        ev.seq,
        ev.kind
    );
    dump_full_state(ev.seq);

    if unsafe { AUTO_DUMP_ON_BREAK } {
        let path = format!("break_{}.json", ev.seq);
        crate::debug::trace::export_json(std::path::Path::new(&path)).ok();
        eprintln!("Dumped trace to {}", path);
    }
}

#[cfg(feature = "debug-instrument")]
fn dump_full_state(at_seq: u64) {
    eprintln!("=== STATE DUMP AT SEQ {} ===", at_seq);

    let window = crate::debug::trace::get_range(at_seq.saturating_sub(10), at_seq + 10);
    eprintln!("Trace window ({} events):", window.len());
    for ev in &window {
        eprintln!("  {:?}", ev);
    }

    eprintln!("=== END DUMP ===");
}

#[cfg(feature = "debug-instrument")]
pub fn get_break_hit_count() -> u64 {
    unsafe { BREAK_HIT_COUNT }
}

#[cfg(feature = "debug-instrument")]
pub fn set_break_on_row_id(row_id: i64) {
    add_break_condition(move |ev| {
        if let crate::debug::trace::EventKind::Insert = ev.kind {
            ev.data
                .insert
                .as_ref()
                .map(|i| i.row_id == row_id)
                .unwrap_or(false)
        } else {
            false
        }
    });
}

#[cfg(feature = "debug-instrument")]
pub fn set_break_on_page(page_no: u32) {
    add_break_condition(move |ev| match ev.kind {
        crate::debug::trace::EventKind::PageRead
        | crate::debug::trace::EventKind::PageWrite
        | crate::debug::trace::EventKind::PageAlloc => ev
            .data
            .page
            .as_ref()
            .map(|p| p.page_no == page_no)
            .unwrap_or(false),
        crate::debug::trace::EventKind::Split => ev
            .data
            .split
            .as_ref()
            .map(|s| s.left == page_no || s.right == page_no || s.parent == page_no)
            .unwrap_or(false),
        _ => false,
    });
}

#[cfg(not(feature = "debug-instrument"))]
pub fn set_break_at_seq(_seq: u64) {}
#[cfg(not(feature = "debug-instrument"))]
pub fn set_break_at_kind(_kind: u8) {}
#[cfg(not(feature = "debug-instrument"))]
pub fn add_break_condition<F>(_f: F)
where
    F: Fn(&crate::debug::trace::TraceEvent) -> bool + 'static,
{
}
#[cfg(not(feature = "debug-instrument"))]
pub fn set_auto_dump(_enabled: bool) {}
#[cfg(not(feature = "debug-instrument"))]
pub fn clear_breakpoints() {}
#[cfg(not(feature = "debug-instrument"))]
pub fn check_breakpoint(_ev: &crate::debug::trace::TraceEvent) {}
#[cfg(not(feature = "debug-instrument"))]
pub fn get_break_hit_count() -> u64 {
    0
}
#[cfg(not(feature = "debug-instrument"))]
pub fn set_break_on_row_id(_row_id: i64) {}
#[cfg(not(feature = "debug-instrument"))]
pub fn set_break_on_page(_page_no: u32) {}
