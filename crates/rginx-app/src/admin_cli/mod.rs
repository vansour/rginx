pub(super) use std::io::{BufReader, Read, Write};
pub(super) use std::os::unix::net::UnixStream;
pub(super) use std::path::Path;
pub(super) use std::time::UNIX_EPOCH;

pub(super) use anyhow::{Context, anyhow};
pub(super) use rginx_runtime::admin::{
    AdminRequest, AdminResponse, RevisionSnapshot, admin_socket_path_for_config,
};

use crate::cli::Command;
pub(super) use crate::cli::{DeltaArgs, SnapshotArgs, SnapshotModuleArg, WaitArgs, WindowArgs};

mod counters;
mod peers;
mod render;
mod snapshot;
mod socket;
mod status;
mod traffic;
mod upstreams;

pub(crate) fn run_admin_command(config_path: &Path, command: &Command) -> anyhow::Result<bool> {
    match command {
        Command::Snapshot(args) => {
            snapshot::print_admin_snapshot(config_path, args)?;
            Ok(true)
        }
        Command::SnapshotVersion => {
            snapshot::print_admin_snapshot_version(config_path)?;
            Ok(true)
        }
        Command::Delta(args) => {
            snapshot::print_admin_delta(config_path, args)?;
            Ok(true)
        }
        Command::Wait(args) => {
            snapshot::print_admin_wait(config_path, args)?;
            Ok(true)
        }
        Command::Status => {
            status::print_admin_status(config_path)?;
            Ok(true)
        }
        Command::Counters => {
            counters::print_admin_counters(config_path)?;
            Ok(true)
        }
        Command::Traffic(args) => {
            traffic::print_admin_traffic(config_path, args)?;
            Ok(true)
        }
        Command::Peers => {
            peers::print_admin_peers(config_path)?;
            Ok(true)
        }
        Command::Upstreams(args) => {
            upstreams::print_admin_upstreams(config_path, args)?;
            Ok(true)
        }
        Command::Check | Command::MigrateNginx(_) => Ok(false),
    }
}
