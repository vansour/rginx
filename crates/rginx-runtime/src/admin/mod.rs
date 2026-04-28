use rginx_http::SharedState;
use tokio::sync::watch;

mod model;
mod service;
mod socket;
#[cfg(test)]
mod tests;

pub use model::{
    AdminRequest, AdminResponse, AdminSnapshot, RevisionSnapshot, SnapshotVersionSnapshot,
};
pub use socket::admin_socket_path_for_config;

const INSTALLED_CONFIG_PATH: &str = "/etc/rginx/rginx.ron";
const INSTALLED_ADMIN_SOCKET_PATH: &str = "/run/rginx/admin.sock";
const ADMIN_SNAPSHOT_SCHEMA_VERSION: u32 = 14;
const DEFAULT_RECENT_WINDOW_SECS: u64 = 60;
const EXTENDED_RECENT_WINDOW_SECS: u64 = 300;

pub async fn run(
    config_path: std::path::PathBuf,
    state: SharedState,
    shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    service::run(config_path, state, shutdown).await
}
