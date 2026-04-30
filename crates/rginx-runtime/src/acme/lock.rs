use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use rginx_core::{AcmeSettings, Error, Result};

const ACME_STATE_LOCK_FILE: &str = ".acme.lock";

#[derive(Debug)]
pub(crate) struct AcmeStateLock {
    _file: File,
    #[cfg(test)]
    path: PathBuf,
}

impl AcmeStateLock {
    pub(crate) fn acquire(settings: &AcmeSettings) -> Result<Self> {
        fs::create_dir_all(&settings.state_dir)?;
        let path = lock_path(settings.state_dir.as_path());
        let file =
            OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path)?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(Self {
                _file: file,
                #[cfg(test)]
                path,
            });
        }

        let error = std::io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN)
        {
            return Err(Error::Server(format!(
                "ACME state directory lock `{}` is already held by another process",
                path.display()
            )));
        }

        Err(Error::Server(format!(
            "failed to acquire ACME state directory lock `{}`: {error}",
            path.display()
        )))
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AcmeStateLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn lock_path(state_dir: &Path) -> PathBuf {
    state_dir.join(ACME_STATE_LOCK_FILE)
}
