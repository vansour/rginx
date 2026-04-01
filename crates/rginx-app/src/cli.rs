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
    for candidate in ["/etc/rginx/rginx.ron", "/usr/local/etc/rginx/rginx.ron"] {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return path;
        }
    }

    PathBuf::from("configs/rginx.ron")
}

fn installed_config_path(executable: &Path) -> Option<PathBuf> {
    executable.parent()?.parent().map(|prefix| prefix.join("etc/rginx/rginx.ron"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::installed_config_path;

    #[test]
    fn installed_config_path_uses_prefix_relative_etc_directory() {
        let executable = PathBuf::from("/usr/local/bin/rginx");

        assert_eq!(
            installed_config_path(&executable),
            Some(PathBuf::from("/usr/local/etc/rginx/rginx.ron"))
        );
    }

    #[test]
    fn installed_config_path_returns_none_for_rootless_paths() {
        let executable = PathBuf::from("rginx");

        assert_eq!(installed_config_path(&executable), None);
    }
}
