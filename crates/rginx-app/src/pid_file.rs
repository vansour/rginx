use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};

const RESTART_READY_FD_ENV: &str = "RGINX_RESTART_READY_FD";

pub(crate) struct PidFileGuard {
    path: PathBuf,
}

pub(crate) struct PidFileRecord {
    pub(crate) pid: i32,
    pub(crate) process_name: Option<String>,
}

impl PidFileGuard {
    pub(crate) fn create(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create pid directory {}", parent.display()))?;
        }

        let mut file = match open_pid_file_for_current_start(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(anyhow!(
                    "pid file {} already exists; another rginx process may already be running",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create pid file {}", path.display()));
            }
        };

        write!(file, "{}\n{}\n", std::process::id(), current_process_name())
            .with_context(|| format!("failed to write pid file {}", path.display()))?;

        Ok(Self { path })
    }
}

pub(crate) fn read_pid_file_record(path: &Path) -> anyhow::Result<PidFileRecord> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read pid file {}", path.display()))?;
    parse_pid_file_record(&contents, path)
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let expected_pid = std::process::id().to_string();
        let current = match fs::read_to_string(&self.path) {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    expected_pid = %expected_pid,
                    error = %error,
                    "failed to read pid file during cleanup"
                );
                return;
            }
        };

        let current_pid = match current.lines().next() {
            Some(pid) => pid.trim(),
            None => {
                tracing::warn!(
                    path = %self.path.display(),
                    expected_pid = %expected_pid,
                    "pid file was empty during cleanup"
                );
                return;
            }
        };

        if current_pid != expected_pid {
            return;
        }

        if let Err(error) = fs::remove_file(&self.path) {
            tracing::warn!(
                path = %self.path.display(),
                expected_pid = %expected_pid,
                current_pid = %current_pid,
                error = %error,
                "failed to remove pid file during cleanup"
            );
        }
    }
}

fn parse_pid_file_record(contents: &str, path: &Path) -> anyhow::Result<PidFileRecord> {
    let mut lines = contents.lines();
    let raw_pid = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or_else(|| anyhow!("pid file {} is empty", path.display()))?;
    let pid = raw_pid
        .parse::<i32>()
        .with_context(|| format!("invalid pid file contents in {}", path.display()))?;
    let process_name =
        lines.next().map(str::trim).filter(|name| !name.is_empty()).map(ToOwned::to_owned);

    Ok(PidFileRecord { pid, process_name })
}

fn current_process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().and_then(|name| name.to_str()).map(ToOwned::to_owned))
        .unwrap_or_else(|| "rginx".to_string())
}

fn open_pid_file_for_current_start(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true);

    if std::env::var_os(RESTART_READY_FD_ENV).is_some() {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }

    options.open(path)
}
