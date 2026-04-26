use std::fs;
use std::path::PathBuf;

use anyhow::Context;

pub(crate) struct PidFileGuard {
    path: PathBuf,
}

impl PidFileGuard {
    pub(crate) fn create(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create pid directory {}", parent.display()))?;
        }

        fs::write(&path, format!("{}\n", std::process::id()))
            .with_context(|| format!("failed to write pid file {}", path.display()))?;

        Ok(Self { path })
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let Ok(current) = fs::read_to_string(&self.path) else {
            return;
        };
        if current.trim() == std::process::id().to_string() {
            let _ = fs::remove_file(&self.path);
        }
    }
}
