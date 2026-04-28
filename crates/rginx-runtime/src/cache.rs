use std::time::Duration;

use rginx_http::SharedState;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

const CACHE_INACTIVE_CLEANUP_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub async fn run(state: SharedState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(CACHE_INACTIVE_CLEANUP_POLL_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval.tick().await;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                if *shutdown.borrow() {
                    break;
                }
                state.cleanup_cache_inactive_entries().await;
            }
        }
    }

    tracing::info!("cache inactive cleanup stopped");
}
