use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn create_temp_dir(name: &str) -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir();

    let path = base.join(name);

    match fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(e) => Err(e),
    }
}
