use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

static SHARED_FILL_STATE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| std::io::Error::other(error.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = temp_json_path(path);
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

pub(super) fn next_shared_fill_nonce(now: u64) -> String {
    let counter = SHARED_FILL_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{now}-{}-{counter}", std::process::id())
}

fn temp_json_path(path: &Path) -> PathBuf {
    let counter = SHARED_FILL_STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "shared-fill-state".into());
    file_name.push(format!(".tmp-{}-{counter}", std::process::id()));
    path.with_file_name(file_name)
}
