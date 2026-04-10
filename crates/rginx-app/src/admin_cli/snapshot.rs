use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_snapshot(config_path: &Path, args: &SnapshotArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetSnapshot {
            include: requested_snapshot_modules(&args.include),
            window_secs: args.window_secs,
        },
    )? {
        AdminResponse::Snapshot(snapshot) => {
            let rendered = serde_json::to_string_pretty(&snapshot)
                .context("failed to encode snapshot JSON")?;
            println!("{rendered}");
            Ok(())
        }
        response => Err(unexpected_admin_response("snapshot", &response)),
    }
}

pub(super) fn print_admin_snapshot_version(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetSnapshotVersion)? {
        AdminResponse::SnapshotVersion(snapshot) => {
            println!("snapshot_version={}", snapshot.snapshot_version);
            Ok(())
        }
        response => Err(unexpected_admin_response("snapshot-version", &response)),
    }
}

pub(super) fn print_admin_delta(config_path: &Path, args: &DeltaArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetDelta {
            since_version: args.since_version,
            include: requested_snapshot_modules(&args.include),
            window_secs: args.window_secs,
        },
    )? {
        AdminResponse::Delta(delta) => {
            let rendered =
                serde_json::to_string_pretty(&delta).context("failed to encode delta JSON")?;
            println!("{rendered}");
            Ok(())
        }
        response => Err(unexpected_admin_response("delta", &response)),
    }
}

pub(super) fn print_admin_wait(config_path: &Path, args: &WaitArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::WaitForSnapshotChange {
            since_version: args.since_version,
            timeout_ms: args.timeout_ms,
        },
    )? {
        AdminResponse::SnapshotVersion(snapshot) => {
            println!("snapshot_version={}", snapshot.snapshot_version);
            Ok(())
        }
        response => Err(unexpected_admin_response("wait", &response)),
    }
}

fn requested_snapshot_modules(
    include: &[SnapshotModuleArg],
) -> Option<Vec<rginx_http::SnapshotModule>> {
    if include.is_empty() {
        return None;
    }

    Some(include.iter().copied().map(snapshot_module).collect())
}

fn snapshot_module(module: SnapshotModuleArg) -> rginx_http::SnapshotModule {
    match module {
        SnapshotModuleArg::Status => rginx_http::SnapshotModule::Status,
        SnapshotModuleArg::Counters => rginx_http::SnapshotModule::Counters,
        SnapshotModuleArg::Traffic => rginx_http::SnapshotModule::Traffic,
        SnapshotModuleArg::PeerHealth => rginx_http::SnapshotModule::PeerHealth,
        SnapshotModuleArg::Upstreams => rginx_http::SnapshotModule::Upstreams,
    }
}
