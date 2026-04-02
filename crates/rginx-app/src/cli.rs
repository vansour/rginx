use std::env;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "rginx",
    version,
    about = "A Rust edge reverse proxy for small and medium deployments"
)]
pub struct Cli {
    #[arg(short, long, global = true, default_value_os_t = default_config_path())]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum Command {
    Check,
}

fn default_config_path() -> PathBuf {
    if let Some(path) = env::var_os("rginx_config") {
        return PathBuf::from(path);
    }

    if let Ok(executable) = env::current_exe()
        && let Some(path) = installed_config_path(&executable)
        && path.exists()
    {
        return path;
    }

    #[cfg(target_os = "linux")]
    for candidate in ["/etc/rginx/rginx.conf", "/etc/rginx/rginx.ron"] {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return path;
        }
    }

    PathBuf::from("configs/rginx.ron")
}

fn installed_config_path(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    let prefix = bin_dir.parent()?;
    let bin_dir_name = bin_dir.file_name()?.to_str()?;

    if prefix != Path::new("/usr") || !matches!(bin_dir_name, "sbin" | "bin") {
        return None;
    }

    Some(PathBuf::from("/etc/rginx/rginx.conf"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::installed_config_path;

    #[test]
    fn installed_config_path_uses_etc_directory_for_usr_sbin_layout() {
        let executable = PathBuf::from("/usr/sbin/rginx");

        assert_eq!(
            installed_config_path(&executable),
            Some(PathBuf::from("/etc/rginx/rginx.conf"))
        );
    }

    #[test]
    fn installed_config_path_returns_none_for_rootless_paths() {
        let executable = PathBuf::from("rginx");

        assert_eq!(installed_config_path(&executable), None);
    }

    #[test]
    fn installed_config_path_returns_none_for_non_system_prefixes() {
        let executable = PathBuf::from("/usr/local/sbin/rginx");

        assert_eq!(installed_config_path(&executable), None);
    }
}
