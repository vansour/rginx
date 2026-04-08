use std::env;
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

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

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Check,
    Snapshot(SnapshotArgs),
    SnapshotVersion,
    Delta(DeltaArgs),
    Wait(WaitArgs),
    Status,
    Counters,
    Traffic(WindowArgs),
    Peers,
    Upstreams(WindowArgs),
    MigrateNginx(MigrateNginxArgs),
}

#[derive(Debug, Clone, Args)]
pub struct MigrateNginxArgs {
    #[arg(long)]
    pub input: PathBuf,

    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct WaitArgs {
    #[arg(long)]
    pub since_version: u64,

    #[arg(long)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct WindowArgs {
    #[arg(long, value_parser = parse_recent_window_secs)]
    pub window_secs: Option<u64>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct SnapshotArgs {
    #[arg(long, value_enum)]
    pub include: Vec<SnapshotModuleArg>,

    #[arg(long, value_parser = parse_recent_window_secs)]
    pub window_secs: Option<u64>,
}

#[derive(Debug, Clone, Args)]
pub struct DeltaArgs {
    #[arg(long)]
    pub since_version: u64,

    #[arg(long, value_enum)]
    pub include: Vec<SnapshotModuleArg>,

    #[arg(long, value_parser = parse_recent_window_secs)]
    pub window_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SnapshotModuleArg {
    Status,
    Counters,
    Traffic,
    #[value(name = "peer-health")]
    PeerHealth,
    Upstreams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignalCommand {
    Reload,
    Restart,
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

fn parse_recent_window_secs(value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|error| format!("invalid recent window `{value}`: {error}"))?;
    match parsed {
        60 | 300 => Ok(parsed),
        _ => Err("recent window must be either 60 or 300 seconds".to_string()),
    }
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

    use super::{
        Cli, Command, DeltaArgs, MigrateNginxArgs, SignalCommand, SnapshotArgs, SnapshotModuleArg,
        WaitArgs, WindowArgs, installed_config_path, pid_path_for_config,
    };

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

    #[test]
    fn cli_accepts_restart_signal_flag() {
        let cli = Cli::try_parse_from(["rginx", "-s", "restart"]).expect("cli should parse");

        assert_eq!(cli.signal, Some(SignalCommand::Restart));
    }

    #[test]
    fn cli_accepts_status_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "status"]).expect("cli should parse");

        assert!(matches!(cli.command, Some(Command::Status)));
    }

    #[test]
    fn cli_accepts_snapshot_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "snapshot"]).expect("cli should parse");

        assert!(
            matches!(cli.command, Some(Command::Snapshot(SnapshotArgs { include, window_secs: None })) if include.is_empty())
        );
    }

    #[test]
    fn cli_accepts_snapshot_version_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "snapshot-version"]).expect("cli should parse");

        assert!(matches!(cli.command, Some(Command::SnapshotVersion)));
    }

    #[test]
    fn cli_accepts_delta_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "delta", "--since-version", "3"])
            .expect("cli should parse");

        assert!(matches!(
            cli.command,
            Some(Command::Delta(DeltaArgs {
                since_version: 3,
                include,
                window_secs: None,
            })) if include.is_empty()
        ));
    }

    #[test]
    fn cli_accepts_snapshot_include_flags() {
        let cli = Cli::try_parse_from([
            "rginx",
            "snapshot",
            "--include",
            "traffic",
            "--include",
            "upstreams",
        ])
        .expect("cli should parse");

        assert!(matches!(
            cli.command,
            Some(Command::Snapshot(SnapshotArgs { include, window_secs: None }))
                if include == vec![SnapshotModuleArg::Traffic, SnapshotModuleArg::Upstreams]
        ));
    }

    #[test]
    fn cli_accepts_window_secs_flags() {
        let cli = Cli::try_parse_from(["rginx", "traffic", "--window-secs", "300"])
            .expect("cli should parse");

        assert!(matches!(
            cli.command,
            Some(Command::Traffic(WindowArgs { window_secs: Some(300) }))
        ));
    }

    #[test]
    fn cli_accepts_wait_subcommand() {
        let cli =
            Cli::try_parse_from(["rginx", "wait", "--since-version", "3", "--timeout-ms", "1000"])
                .expect("cli should parse");

        assert!(matches!(
            cli.command,
            Some(Command::Wait(WaitArgs { since_version: 3, timeout_ms: Some(1000) }))
        ));
    }

    #[test]
    fn cli_accepts_traffic_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "traffic"]).expect("cli should parse");

        assert!(matches!(cli.command, Some(Command::Traffic(WindowArgs { window_secs: None }))));
    }

    #[test]
    fn cli_accepts_upstreams_subcommand() {
        let cli = Cli::try_parse_from(["rginx", "upstreams"]).expect("cli should parse");

        assert!(matches!(cli.command, Some(Command::Upstreams(WindowArgs { window_secs: None }))));
    }

    #[test]
    fn cli_accepts_migrate_nginx_subcommand() {
        let cli = Cli::try_parse_from([
            "rginx",
            "migrate-nginx",
            "--input",
            "nginx.conf",
            "--output",
            "rginx.ron",
        ])
        .expect("cli should parse");

        assert!(matches!(cli.command, Some(Command::MigrateNginx(MigrateNginxArgs { .. }))));
    }
}
