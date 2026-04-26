use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rginx_http::{SharedState, SnapshotModule};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::model::{
    AdminRequest, AdminResponse, AdminSnapshot, RevisionSnapshot, SnapshotVersionSnapshot,
};
use super::socket::{
    AdminSocketGuard, admin_socket_path_for_config, log_admin_connection_result,
    set_admin_socket_permissions,
};
use super::{
    ADMIN_SNAPSHOT_SCHEMA_VERSION, DEFAULT_RECENT_WINDOW_SECS, EXTENDED_RECENT_WINDOW_SECS,
};

pub(super) async fn run(
    config_path: PathBuf,
    state: SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let socket_path = admin_socket_path_for_config(&config_path);
    AdminSocketGuard::prepare_path(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)?;
    let _guard = AdminSocketGuard::from_bound_path(&socket_path)?;
    set_admin_socket_permissions(&socket_path)?;
    let mut connections = JoinSet::new();

    tracing::info!(path = %socket_path.display(), "local admin socket listening");

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => break,
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            accepted = listener.accept() => {
                while let Some(result) = connections.try_join_next() {
                    log_admin_connection_result(result);
                }

                let (stream, _) = accepted?;
                let state = state.clone();
                connections.spawn(async move {
                    if let Err(error) = handle_connection(stream, state).await {
                        tracing::warn!(%error, "admin socket request failed");
                    }
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = joined {
                    log_admin_connection_result(result);
                }
            }
        }
    }

    while let Some(result) = connections.join_next().await {
        log_admin_connection_result(result);
    }

    tracing::info!(path = %socket_path.display(), "local admin socket stopped");

    Ok(())
}

async fn handle_connection(stream: UnixStream, state: SharedState) -> io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut request = String::new();

    let bytes = reader.read_line(&mut request).await?;
    if bytes == 0 {
        return Ok(());
    }

    let request = serde_json::from_str::<AdminRequest>(request.trim_end()).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("invalid admin request: {error}"))
    })?;

    let response = dispatch_request(request, &state).await?;
    let encoded = serde_json::to_vec(&response)
        .map_err(|error| io::Error::other(format!("invalid admin response: {error}")))?;
    writer.write_all(&encoded).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

async fn dispatch_request(request: AdminRequest, state: &SharedState) -> io::Result<AdminResponse> {
    Ok(match request {
        AdminRequest::GetSnapshot { include, window_secs } => {
            let window_secs = normalize_recent_window_secs(window_secs)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
            let included_modules = SnapshotModule::normalize(include.as_deref());
            let status = if included_modules.contains(&SnapshotModule::Status) {
                Some(state.status_snapshot().await)
            } else {
                None
            };
            let counters = included_modules
                .contains(&SnapshotModule::Counters)
                .then(|| state.counters_snapshot());
            let traffic = included_modules
                .contains(&SnapshotModule::Traffic)
                .then(|| state.traffic_stats_snapshot_with_window(window_secs));
            let peer_health = if included_modules.contains(&SnapshotModule::PeerHealth) {
                Some(state.peer_health_snapshot().await)
            } else {
                None
            };
            let upstreams = included_modules
                .contains(&SnapshotModule::Upstreams)
                .then(|| state.upstream_stats_snapshot_with_window(window_secs));
            AdminResponse::Snapshot(AdminSnapshot {
                schema_version: ADMIN_SNAPSHOT_SCHEMA_VERSION,
                snapshot_version: state.current_snapshot_version(),
                captured_at_unix_ms: unix_time_ms(SystemTime::now()),
                pid: std::process::id(),
                binary_version: env!("CARGO_PKG_VERSION").to_string(),
                included_modules: included_modules.clone(),
                status,
                counters,
                traffic,
                peer_health,
                upstreams,
            })
        }
        AdminRequest::GetSnapshotVersion => {
            AdminResponse::SnapshotVersion(SnapshotVersionSnapshot {
                snapshot_version: state.current_snapshot_version(),
            })
        }
        AdminRequest::GetDelta { since_version, include, window_secs } => {
            let window_secs = normalize_recent_window_secs(window_secs)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
            AdminResponse::Delta(state.snapshot_delta_since(
                since_version,
                include.as_deref(),
                window_secs,
            ))
        }
        AdminRequest::WaitForSnapshotChange { since_version, timeout_ms } => {
            let timeout = timeout_ms.map(std::time::Duration::from_millis);
            let snapshot_version = state.wait_for_snapshot_change(since_version, timeout).await;
            AdminResponse::SnapshotVersion(SnapshotVersionSnapshot { snapshot_version })
        }
        AdminRequest::GetStatus => AdminResponse::Status(state.status_snapshot().await),
        AdminRequest::GetCounters => AdminResponse::Counters(state.counters_snapshot()),
        AdminRequest::GetTrafficStats { window_secs } => {
            let window_secs = normalize_recent_window_secs(window_secs)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
            AdminResponse::TrafficStats(state.traffic_stats_snapshot_with_window(window_secs))
        }
        AdminRequest::GetPeerHealth => {
            AdminResponse::PeerHealth(state.peer_health_snapshot().await)
        }
        AdminRequest::GetUpstreamStats { window_secs } => {
            let window_secs = normalize_recent_window_secs(window_secs)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
            AdminResponse::UpstreamStats(state.upstream_stats_snapshot_with_window(window_secs))
        }
        AdminRequest::GetRevision => {
            AdminResponse::Revision(RevisionSnapshot { revision: state.current_revision().await })
        }
    })
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn normalize_recent_window_secs(window_secs: Option<u64>) -> Result<Option<u64>, String> {
    match window_secs {
        None => Ok(None),
        Some(DEFAULT_RECENT_WINDOW_SECS) => Ok(Some(DEFAULT_RECENT_WINDOW_SECS)),
        Some(EXTENDED_RECENT_WINDOW_SECS) => Ok(Some(EXTENDED_RECENT_WINDOW_SECS)),
        Some(other) => Err(format!(
            "unsupported recent window `{other}`; only {DEFAULT_RECENT_WINDOW_SECS} or {EXTENDED_RECENT_WINDOW_SECS} seconds are supported"
        )),
    }
}
