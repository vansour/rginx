use std::env;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "rginx",
    version,
    about = "A Rust edge reverse proxy for small and medium deployments"
)]
pub struct Cli {
    #[arg(short, long, global = true, default_value_os_t = default_config_path())]
    pub config: PathBuf,

    #[arg(short = 't', action = ArgAction::SetTrue, conflicts_with = "signal")]
    pub test_config: bool,

    #[arg(short = 's', value_enum, conflicts_with = "test_config")]
    pub signal: Option<SignalCommand>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum Command {
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignalCommand {
    Reload,
    Stop,
    Quit,
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
    {
        let path = PathBuf::from("/etc/rginx/rginx.ron");
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

    Some(PathBuf::from("/etc/rginx/rginx.ron"))
}

pub fn pid_path_for_config(config_path: &Path) -> PathBuf {
    if config_path == Path::new("/etc/rginx/rginx.ron") {
        return PathBuf::from("/run/rginx.pid");
    }

    config_path.with_extension("pid")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use clap::Parser;

    use super::{Cli, SignalCommand, installed_config_path, pid_path_for_config};

    #[test]
    fn installed_config_path_uses_etc_directory_for_usr_sbin_layout() {
        let executable = PathBuf::from("/usr/sbin/rginx");

        assert_eq!(installed_config_path(&executable), Some(PathBuf::from("/etc/rginx/rginx.ron")));
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

    #[test]
    fn pid_path_for_installed_config_uses_run_directory() {
        assert_eq!(
            pid_path_for_config(Path::new("/etc/rginx/rginx.ron")),
            PathBuf::from("/run/rginx.pid")
        );
    }

    #[test]
    fn pid_path_for_custom_config_uses_matching_pid_file() {
        assert_eq!(pid_path_for_config(Path::new("/tmp/site.ron")), PathBuf::from("/tmp/site.pid"));
    }

    #[test]
    fn cli_accepts_nginx_style_t_flag() {
        let cli = Cli::try_parse_from(["rginx", "-t"]).expect("cli should parse");

        assert!(cli.test_config);
        assert!(cli.signal.is_none());
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_accepts_nginx_style_signal_flag() {
        let cli = Cli::try_parse_from(["rginx", "-s", "reload"]).expect("cli should parse");

        assert!(!cli.test_config);
        assert_eq!(cli.signal, Some(SignalCommand::Reload));
        assert!(cli.command.is_none());
    }
}
