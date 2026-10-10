use std::fs;
use std::path::PathBuf;

/// Create the directory `name` under the system temporary location, or return
/// the existing one.
///
/// The in-memory VFS calls this to give its buffers a directory, so code that
/// asks a file for its [`path`](crate::vfs::file::InkFile::path) gets something
/// real even when the bytes themselves never touch the disk.
pub fn create_temp_dir(name: &str) -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir();

    let path = base.join(name);

    match fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(e) => Err(e),
    }
}
