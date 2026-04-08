use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rginx_http::{
    HttpCountersSnapshot, RuntimeStatusSnapshot, SharedState, SnapshotDeltaSnapshot,
    SnapshotModule, TrafficStatsSnapshot, UpstreamHealthSnapshot, UpstreamStatsSnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};

const INSTALLED_CONFIG_PATH: &str = "/etc/rginx/rginx.ron";
const INSTALLED_ADMIN_SOCKET_PATH: &str = "/run/rginx/admin.sock";
const ADMIN_SNAPSHOT_SCHEMA_VERSION: u32 = 7;
const DEFAULT_RECENT_WINDOW_SECS: u64 = 60;
const EXTENDED_RECENT_WINDOW_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    GetSnapshot { include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    GetSnapshotVersion,
    GetDelta { since_version: u64, include: Option<Vec<SnapshotModule>>, window_secs: Option<u64> },
    WaitForSnapshotChange { since_version: u64, timeout_ms: Option<u64> },
    GetStatus,
    GetCounters,
    GetTrafficStats { window_secs: Option<u64> },
    GetPeerHealth,
    GetUpstreamStats { window_secs: Option<u64> },
    GetRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionSnapshot {
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotVersionSnapshot {
    pub snapshot_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminSnapshot {
    pub schema_version: u32,
    pub snapshot_version: u64,
    pub captured_at_unix_ms: u64,
    pub pid: u32,
    pub binary_version: String,
    pub included_modules: Vec<SnapshotModule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<RuntimeStatusSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters: Option<HttpCountersSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficStatsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_health: Option<Vec<UpstreamHealthSnapshot>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstreams: Option<Vec<UpstreamStatsSnapshot>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AdminResponse {
    Snapshot(AdminSnapshot),
    SnapshotVersion(SnapshotVersionSnapshot),
    Delta(SnapshotDeltaSnapshot),
    Status(RuntimeStatusSnapshot),
    Counters(HttpCountersSnapshot),
    TrafficStats(TrafficStatsSnapshot),
    PeerHealth(Vec<UpstreamHealthSnapshot>),
    UpstreamStats(Vec<UpstreamStatsSnapshot>),
    Revision(RevisionSnapshot),
    Error { message: String },
}

pub fn admin_socket_path_for_config(config_path: &Path) -> PathBuf {
    if config_path == Path::new(INSTALLED_CONFIG_PATH) {
        return PathBuf::from(INSTALLED_ADMIN_SOCKET_PATH);
    }

    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = config_path.file_stem().and_then(|value| value.to_str()).unwrap_or("rginx");
    parent.join(format!("{stem}.admin.sock"))
}

pub async fn run(
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

    let response = match request {
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
    };

    let encoded = serde_json::to_vec(&response).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("invalid admin response: {error}"))
    })?;
    writer.write_all(&encoded).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

struct AdminSocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl AdminSocketGuard {
    fn prepare_path(path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    fn from_bound_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self { path: path.to_path_buf(), device: metadata.dev(), inode: metadata.ino() })
    }
}

impl Drop for AdminSocketGuard {
    fn drop(&mut self) {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return;
        };
        if metadata.dev() == self.device && metadata.ino() == self.inode {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn log_admin_connection_result(result: Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_panic() {
            tracing::warn!(%error, "admin socket task panicked");
        } else if !error.is_cancelled() {
            tracing::warn!(%error, "admin socket task failed to join");
        }
    }
}

fn set_admin_socket_permissions(path: &Path) -> io::Result<()> {
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{INSTALLED_ADMIN_SOCKET_PATH, admin_socket_path_for_config};

    #[test]
    fn installed_config_uses_run_admin_socket() {
        assert_eq!(
            admin_socket_path_for_config(Path::new("/etc/rginx/rginx.ron")),
            PathBuf::from(INSTALLED_ADMIN_SOCKET_PATH)
        );
    }

    #[test]
    fn custom_config_uses_neighbor_admin_socket() {
        assert_eq!(
            admin_socket_path_for_config(Path::new("/tmp/site.ron")),
            PathBuf::from("/tmp/site.admin.sock")
        );
    }
}
