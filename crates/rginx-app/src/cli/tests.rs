use std::path::{Path, PathBuf};

use clap::Parser;

use super::{
    Cli, Command, DeltaArgs, PurgeCacheArgs, SignalCommand, SnapshotArgs, SnapshotModuleArg,
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
        PathBuf::from("/run/rginx/rginx.pid")
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
    let cli =
        Cli::try_parse_from(["rginx", "delta", "--since-version", "3"]).expect("cli should parse");

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
fn cli_accepts_cache_snapshot_module() {
    let cli = Cli::try_parse_from(["rginx", "snapshot", "--include", "cache"])
        .expect("cli should parse");

    assert!(matches!(
        cli.command,
        Some(Command::Snapshot(SnapshotArgs { include, window_secs: None }))
            if include == vec![SnapshotModuleArg::Cache]
    ));
}

#[test]
fn cli_accepts_window_secs_flags() {
    let cli = Cli::try_parse_from(["rginx", "traffic", "--window-secs", "300"])
        .expect("cli should parse");

    assert!(matches!(cli.command, Some(Command::Traffic(WindowArgs { window_secs: Some(300) }))));
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
fn cli_accepts_cache_subcommand() {
    let cli = Cli::try_parse_from(["rginx", "cache"]).expect("cli should parse");

    assert!(matches!(cli.command, Some(Command::Cache)));
}

#[test]
fn cli_accepts_purge_cache_key_subcommand() {
    let cli = Cli::try_parse_from([
        "rginx",
        "purge-cache",
        "--zone",
        "default",
        "--key",
        "http:example.com:/demo",
    ])
    .expect("cli should parse");

    assert!(matches!(
        cli.command,
        Some(Command::PurgeCache(PurgeCacheArgs { zone, key: Some(key), prefix: None }))
            if zone == "default" && key == "http:example.com:/demo"
    ));
}

#[test]
fn cli_accepts_purge_cache_prefix_subcommand() {
    let cli = Cli::try_parse_from([
        "rginx",
        "purge-cache",
        "--zone",
        "default",
        "--prefix",
        "http:example.com:/assets/",
    ])
    .expect("cli should parse");

    assert!(matches!(
        cli.command,
        Some(Command::PurgeCache(PurgeCacheArgs { zone, key: None, prefix: Some(prefix) }))
            if zone == "default" && prefix == "http:example.com:/assets/"
    ));
}
