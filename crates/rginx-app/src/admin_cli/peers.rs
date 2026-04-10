use super::render::print_record;
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_peers(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetPeerHealth)? {
        AdminResponse::PeerHealth(upstreams) => {
            for upstream in upstreams {
                let upstream_name = upstream.upstream_name.clone();
                print_record(
                    "peer_health_upstream",
                    [
                        ("upstream", upstream_name.clone()),
                        ("unhealthy_after_failures", upstream.unhealthy_after_failures.to_string()),
                        ("cooldown_ms", upstream.cooldown_ms.to_string()),
                        ("active_health_enabled", upstream.active_health_enabled.to_string()),
                    ],
                );
                for peer in upstream.peers {
                    print_record(
                        "peer_health_peer",
                        [
                            ("upstream", upstream_name.clone()),
                            ("peer", peer.peer_url),
                            ("backup", peer.backup.to_string()),
                            ("weight", peer.weight.to_string()),
                            ("available", peer.available.to_string()),
                            ("passive_failures", peer.passive_consecutive_failures.to_string()),
                            (
                                "passive_cooldown_remaining_ms",
                                peer.passive_cooldown_remaining_ms
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            ("passive_pending_recovery", peer.passive_pending_recovery.to_string()),
                            ("active_unhealthy", peer.active_unhealthy.to_string()),
                            ("active_successes", peer.active_consecutive_successes.to_string()),
                            ("active_requests", peer.active_requests.to_string()),
                        ],
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("peers", &response)),
    }
}
