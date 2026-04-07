use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rginx_http::{
    HttpCountersSnapshot, RuntimeStatusSnapshot, SharedState, UpstreamHealthSnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};

const INSTALLED_CONFIG_PATH: &str = "/etc/rginx/rginx.ron";
const INSTALLED_ADMIN_SOCKET_PATH: &str = "/run/rginx/admin.sock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    GetStatus,
    GetCounters,
    GetPeerHealth,
    GetRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionSnapshot {
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AdminResponse {
    Status(RuntimeStatusSnapshot),
    Counters(HttpCountersSnapshot),
    PeerHealth(Vec<UpstreamHealthSnapshot>),
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
    let _guard = AdminSocketGuard::bind_path(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)?;
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
        AdminRequest::GetStatus => AdminResponse::Status(state.status_snapshot().await),
        AdminRequest::GetCounters => AdminResponse::Counters(state.counters_snapshot()),
        AdminRequest::GetPeerHealth => {
            AdminResponse::PeerHealth(state.peer_health_snapshot().await)
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
}

impl AdminSocketGuard {
    fn bind_path(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(Self { path: path.to_path_buf() })
    }
}

impl Drop for AdminSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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
