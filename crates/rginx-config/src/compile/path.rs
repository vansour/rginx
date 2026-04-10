use std::path::{Path, PathBuf};

pub(super) fn resolve_path(base_dir: &Path, path: String) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() { path } else { base_dir.join(path) }
}
