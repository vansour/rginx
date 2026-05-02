use std::path::{Path, PathBuf};

use tokio::fs;

use super::temp::{cleanup_failed_write, next_temp_suffix, sibling_temp_path};
use super::*;

#[cfg(test)]
pub(in crate::cache) async fn write_cache_entry(
    paths: &CachePaths,
    metadata: &CacheMetadata,
    body: &[u8],
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let body_tmp = cache_entry_temp_body_path(paths);

    if let Err(error) = fs::write(&body_tmp, body).await {
        let _ = fs::remove_file(&body_tmp).await;
        return Err(error);
    }
    commit_cache_entry_temp_body(paths, metadata, &body_tmp).await
}

pub(in crate::cache) fn cache_entry_temp_body_path(paths: &CachePaths) -> PathBuf {
    sibling_temp_path(&paths.body, &next_temp_suffix())
}

pub(in crate::cache) async fn commit_cache_entry_temp_body(
    paths: &CachePaths,
    metadata: &CacheMetadata,
    body_tmp: &Path,
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let suffix = next_temp_suffix();
    let metadata_tmp = sibling_temp_path(&paths.metadata, &suffix);
    let metadata_bytes =
        serde_json::to_vec(metadata).map_err(|error| std::io::Error::other(error.to_string()))?;

    if let Err(error) = fs::write(&metadata_tmp, metadata_bytes).await {
        cleanup_failed_write(paths, body_tmp, &metadata_tmp, false).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(body_tmp, &paths.body).await {
        cleanup_failed_write(paths, body_tmp, &metadata_tmp, false).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(&metadata_tmp, &paths.metadata).await {
        cleanup_failed_write(paths, body_tmp, &metadata_tmp, true).await;
        return Err(error);
    }
    Ok(())
}

pub(in crate::cache) async fn write_cache_metadata(
    paths: &CachePaths,
    metadata: &CacheMetadata,
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let suffix = next_temp_suffix();
    let metadata_tmp = sibling_temp_path(&paths.metadata, &suffix);
    let metadata_bytes =
        serde_json::to_vec(metadata).map_err(|error| std::io::Error::other(error.to_string()))?;
    if let Err(error) = fs::write(&metadata_tmp, metadata_bytes).await {
        let _ = fs::remove_file(&metadata_tmp).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(&metadata_tmp, &paths.metadata).await {
        let _ = fs::remove_file(&metadata_tmp).await;
        return Err(error);
    }
    Ok(())
}
