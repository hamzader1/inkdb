#![allow(unused)]

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "debug")]
thread_local! {
    static DEBUG_FILE: RefCell<Option<BufWriter<std::fs::File>>> = RefCell::new(None);
    static DEBUG_PATH: RefCell<Option<PathBuf>> = RefCell::new(None);
}

static INIT: Once = Once::new();

#[cfg(feature = "debug")]
fn init_debug_file(db_path: Option<&Path>) {
    INIT.call_once(|| {
        let path = db_path
            .map(|p| p.with_file_name("debug.log"))
            .unwrap_or_else(|| PathBuf::from("debug.log"));

        DEBUG_PATH.with(|p| *p.borrow_mut() = Some(path.clone()));

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("Failed to create debug.log");

        DEBUG_FILE.with(|f| *f.borrow_mut() = Some(BufWriter::new(file)));

        log_raw(&format!("Debug session started at {}\n", timestamp()));
    });
}

#[cfg(feature = "debug")]
pub fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let datetime = chrono::DateTime::<chrono::Local>::from(SystemTime::now());
    format!("{}", datetime.format("%Y-%m-%d %H:%M:%S%.3f"))
}

#[cfg(feature = "debug")]
pub fn log_raw(msg: &str) {
    DEBUG_FILE.with(|f| {
        if let Some(writer) = f.borrow_mut().as_mut() {
            let _ = writer.write_all(msg.as_bytes());
            let _ = writer.flush();
        }
    });
}

#[cfg(feature = "debug")]
pub fn log_enter(fn_name: &str) {
    log_raw(&format!("[{}] ENTER: @{} function\n", timestamp(), fn_name));
}

#[cfg(feature = "debug")]
pub fn log_exit(fn_name: &str) {
    log_raw(&format!("[{}] EXIT:  @{} function\n", timestamp(), fn_name));
}

/// Main debug log macro - use this anywhere in your code
#[cfg(feature = "debug")]
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {{
        let msg = format!("[{}] {}\n", $crate::debug::timestamp(), format!($($arg)*));
        $crate::debug::log_raw(&msg);
    }};
}

/// Debug log with function context
#[cfg(feature = "debug")]
#[macro_export]
macro_rules! debug_log_fn {
    ($fn_name:expr, $($arg:tt)*) => {{
        let msg = format!("[{}] [{}] {}\n", $crate::debug::timestamp(), $fn_name, format!($($arg)*));
        $crate::debug::log_raw(&msg);
    }};
}

#[cfg(not(feature = "debug"))]
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {};
}

#[cfg(not(feature = "debug"))]
#[macro_export]
macro_rules! debug_log_fn {
    ($fn_name:expr, $($arg:tt)*) => {};
}

/// Initialize debug logging - call once at startup with database path
#[cfg(feature = "debug")]
pub fn init(db_path: Option<&Path>) {
    init_debug_file(db_path);
}

#[cfg(not(feature = "debug"))]
pub fn init(_db_path: Option<&Path>) {}

/// Get the debug log path
#[cfg(feature = "debug")]
pub fn get_debug_path() -> Option<PathBuf> {
    DEBUG_PATH.with(|p| p.borrow().clone())
}

#[cfg(not(feature = "debug"))]
pub fn get_debug_path() -> Option<PathBuf> {
    None
}
