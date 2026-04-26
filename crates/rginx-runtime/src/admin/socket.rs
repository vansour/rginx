use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::task::JoinError;

use super::{INSTALLED_ADMIN_SOCKET_PATH, INSTALLED_CONFIG_PATH};

pub fn admin_socket_path_for_config(config_path: &Path) -> PathBuf {
    if config_path == Path::new(INSTALLED_CONFIG_PATH) {
        return PathBuf::from(INSTALLED_ADMIN_SOCKET_PATH);
    }

    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = config_path.file_stem().and_then(|value| value.to_str()).unwrap_or("rginx");
    parent.join(format!("{stem}.admin.sock"))
}

pub(super) struct AdminSocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl AdminSocketGuard {
    pub(super) fn prepare_path(path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    pub(super) fn from_bound_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self { path: path.to_path_buf(), device: metadata.dev(), inode: metadata.ino() })
    }
}

impl Drop for AdminSocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return;
        };
        if metadata.dev() == self.device && metadata.ino() == self.inode {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn log_admin_connection_result(result: Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_panic() {
            tracing::warn!(%error, "admin socket task panicked");
        } else if !error.is_cancelled() {
            tracing::warn!(%error, "admin socket task failed to join");
        }
    }
}

pub(super) fn set_admin_socket_permissions(path: &Path) -> io::Result<()> {
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
}
