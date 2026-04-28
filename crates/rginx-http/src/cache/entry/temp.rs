use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::fs;

use super::CachePaths;

static CACHE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn next_temp_suffix() -> String {
    let counter = CACHE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".tmp-{}-{counter}", std::process::id())
}

pub(super) fn sibling_temp_path(path: &Path, suffix: &str) -> PathBuf {
    let mut file_name =
        path.file_name().map_or_else(|| OsString::from("cache-entry"), |name| name.to_os_string());
    file_name.push(suffix);
    path.with_file_name(file_name)
}

pub(super) async fn cleanup_failed_write(
    paths: &CachePaths,
    body_tmp: &Path,
    metadata_tmp: &Path,
    remove_final: bool,
) {
    let _ = fs::remove_file(body_tmp).await;
    let _ = fs::remove_file(metadata_tmp).await;
    if remove_final {
        let _ = fs::remove_file(&paths.body).await;
        let _ = fs::remove_file(&paths.metadata).await;
    }
}
