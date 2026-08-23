#[cfg(feature = "debug-instrument")]
use std::cell::RefCell;
#[cfg(feature = "debug-instrument")]
use std::rc::Rc;

#[cfg(feature = "debug-instrument")]
thread_local! {
    static SEQ: RefCell<u64> = RefCell::new(0);
    static CTX_STACK: RefCell<Vec<u64>> = RefCell::new(Vec::new());
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn next_seq() -> u64 {
    SEQ.with(|s| {
        let mut seq = s.borrow_mut();
        let current = *seq;
        *seq += 1;
        current
    })
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn current_seq() -> u64 {
    SEQ.with(|s| *s.borrow())
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn set_seq(seq: u64) {
    SEQ.with(|s| *s.borrow_mut() = seq);
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn push_ctx(seq: u64) {
    CTX_STACK.with(|s| s.borrow_mut().push(seq));
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn pop_ctx() {
    CTX_STACK.with(|s| {
        s.borrow_mut().pop();
    });
}

#[cfg(feature = "debug-instrument")]
#[inline(always)]
pub fn current_ctx() -> Option<u64> {
    CTX_STACK.with(|s| s.borrow().last().copied())
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn next_seq() -> u64 {
    0
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn current_seq() -> u64 {
    0
}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn set_seq(_seq: u64) {}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn push_ctx(_seq: u64) {}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn pop_ctx() {}

#[cfg(not(feature = "debug-instrument"))]
#[inline(always)]
pub fn current_ctx() -> Option<u64> {
    None
}
