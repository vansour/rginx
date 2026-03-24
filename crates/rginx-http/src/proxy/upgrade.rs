use super::health::ActivePeerGuard;
use super::*;

pub(super) async fn proxy_upgraded_connection(
    metrics: Metrics,
    downstream_upgrade: OnUpgrade,
    upstream_upgrade: OnUpgrade,
    upstream_name: String,
    peer_url: String,
    _active_peer: ActivePeerGuard,
) {
    let _guard = ActiveConnectionGuard::new(metrics);

    let (downstream_upgraded, upstream_upgraded) =
        match tokio::try_join!(downstream_upgrade, upstream_upgrade) {
            Ok(upgraded) => upgraded,
            Err(error) => {
                tracing::warn!(
                    upstream = %upstream_name,
                    peer = %peer_url,
                    %error,
                    "failed to complete upgraded proxy handshake"
                );
                return;
            }
        };

    let mut downstream_io = TokioIo::new(downstream_upgraded);
    let mut upstream_io = TokioIo::new(upstream_upgraded);

    match copy_bidirectional(&mut downstream_io, &mut upstream_io).await {
        Ok((from_client_bytes, from_upstream_bytes)) => {
            tracing::info!(
                upstream = %upstream_name,
                peer = %peer_url,
                from_client_bytes,
                from_upstream_bytes,
                "upgraded proxy tunnel closed"
            );
        }
        Err(error) => {
            tracing::warn!(
                upstream = %upstream_name,
                peer = %peer_url,
                %error,
                "upgraded proxy tunnel failed"
            );
        }
    }
}

struct ActiveConnectionGuard {
    metrics: Metrics,
}

impl ActiveConnectionGuard {
    fn new(metrics: Metrics) -> Self {
        Self { metrics }
    }
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        self.metrics.decrement_active_connections();
    }
}
