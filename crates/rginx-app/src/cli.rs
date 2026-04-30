use std::env;
use std::path::{Path, PathBuf};

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, ValueEnum};

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
    Acme(AcmeArgs),
    Check,
    Snapshot(SnapshotArgs),
    SnapshotVersion,
    Delta(DeltaArgs),
    Wait(WaitArgs),
    Status,
    Cache,
    PurgeCache(PurgeCacheArgs),
    Counters,
    Traffic(WindowArgs),
    Peers,
    Upstreams(WindowArgs),
}

#[derive(Debug, Clone, Args)]
pub struct AcmeArgs {
    #[command(subcommand)]
    pub command: AcmeCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AcmeCommand {
    Issue(AcmeIssueArgs),
}

#[derive(Debug, Clone, Args)]
#[command(group = ArgGroup::new("mode").required(true).args(["once"]))]
pub struct AcmeIssueArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    pub once: bool,
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

#[derive(Debug, Clone, Args)]
#[command(group = ArgGroup::new("selector").args(["key", "prefix"]).multiple(false))]
pub struct PurgeCacheArgs {
    #[arg(long)]
    pub zone: String,

    #[arg(long)]
    pub key: Option<String>,

    #[arg(long)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SnapshotModuleArg {
    Status,
    Counters,
    Traffic,
    #[value(name = "peer-health")]
    PeerHealth,
    Upstreams,
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignalCommand {
    Reload,
    Restart,
    Stop,
    Quit,
}

impl SignalCommand {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reload => "reload",
            Self::Restart => "restart",
            Self::Stop => "stop",
            Self::Quit => "quit",
        }
    }
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
        return PathBuf::from("/run/rginx/rginx.pid");
    }

    config_path.with_extension("pid")
}

#[cfg(test)]
mod tests;
