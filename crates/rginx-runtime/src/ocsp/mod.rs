use std::time::Duration;

use client::{OcspClient, build_ocsp_client};
use persist::{handle_ocsp_refresh_failure, write_ocsp_cache_file};
use refresh::fetch_ocsp_response;
#[cfg(test)]
use refresh::fetch_ocsp_response_from_url;

use rginx_http::SharedState;
use tokio::sync::watch;

mod client;
mod persist;
mod refresh;
mod scheduler;
mod spec;
mod state;
#[cfg(test)]
mod tests;

const OCSP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const OCSP_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn run(state: SharedState, shutdown: watch::Receiver<bool>) {
    scheduler::run(state, shutdown).await;
}

/// Refreshes OCSP staples immediately and returns whether callers should rebuild TLS acceptors
/// to pick up changed staple files. This helper does not rebuild TLS acceptors by itself.
pub async fn refresh_now(state: &SharedState) -> Result<bool, String> {
    scheduler::refresh_now(state).await
}
