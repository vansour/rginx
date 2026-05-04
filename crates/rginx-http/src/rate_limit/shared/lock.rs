use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct FileLock {
    file: File,
}

pub(super) fn acquire(lock_path: &Path, contention_total: &AtomicU64) -> io::Result<FileLock> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file =
        OpenOptions::new().create(true).truncate(false).read(true).write(true).open(lock_path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(FileLock { file });
    }

    let error = io::Error::last_os_error();
    let would_block = error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error().is_some_and(|code| code == libc::EWOULDBLOCK);
    if !would_block {
        return Err(error);
    }

    contention_total.fetch_add(1, Ordering::Relaxed);
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
        return Ok(FileLock { file });
    }
    Err(io::Error::last_os_error())
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}
