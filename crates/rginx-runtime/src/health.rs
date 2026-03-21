use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rginx_core::{ActiveHealthCheck, Upstream, UpstreamPeer};
use rginx_http::proxy::{probe_upstream_peer, ProxyClients};
use rginx_http::state::ActiveState;
use rginx_http::SharedState;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProbeKey {
    upstream_name: String,
    peer_url: String,
}

#[derive(Clone)]
struct ProbeTarget {
    key: ProbeKey,
    clients: ProxyClients,
    upstream: Arc<Upstream>,
    peer: UpstreamPeer,
    health_check: ActiveHealthCheck,
}

pub async fn run(state: SharedState, mut shutdown: watch::Receiver<bool>) {
    let mut config_updates = state.subscribe_updates();
    let metrics = state.metrics();
    let mut next_due = HashMap::<ProbeKey, Instant>::new();

    loop {
        if *shutdown.borrow() {
            break;
        }

        let snapshot = state.snapshot().await;
        let now = Instant::now();
        let targets = collect_probe_targets(snapshot);
        let active_keys = targets.iter().map(|target| target.key.clone()).collect::<HashSet<_>>();
        next_due.retain(|key, _| active_keys.contains(key));

        for target in &targets {
            next_due.entry(target.key.clone()).or_insert(now);
        }

        let due_targets = targets
            .into_iter()
            .filter(|target| next_due.get(&target.key).is_some_and(|due_at| *due_at <= now))
            .collect::<Vec<_>>();

        if !due_targets.is_empty() {
            let mut probes = JoinSet::new();

            for target in due_targets {
                next_due.insert(target.key.clone(), now + target.health_check.interval);
                let metrics = metrics.clone();
                probes.spawn(async move {
                    probe_upstream_peer(target.clients, metrics, target.upstream, target.peer)
                        .await;
                });
            }

            while let Some(result) = probes.join_next().await {
                log_probe_task_result(result);
            }

            continue;
        }

        if let Some(soonest_due) = next_due.values().min().copied() {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                changed = config_updates.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    next_due.clear();
                }
                _ = tokio::time::sleep_until(soonest_due) => {}
            }
        } else {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                changed = config_updates.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    next_due.clear();
                }
            }
        }
    }

    tracing::info!("active health checker stopped");
}

fn collect_probe_targets(snapshot: ActiveState) -> Vec<ProbeTarget> {
    let clients = snapshot.clients;
    let mut targets = Vec::new();

    for upstream in snapshot.config.upstreams.values() {
        let Some(health_check) = upstream.active_health_check.clone() else {
            continue;
        };

        for peer in &upstream.peers {
            targets.push(ProbeTarget {
                key: ProbeKey { upstream_name: upstream.name.clone(), peer_url: peer.url.clone() },
                clients: clients.clone(),
                upstream: upstream.clone(),
                peer: peer.clone(),
                health_check: health_check.clone(),
            });
        }
    }

    targets
}

fn log_probe_task_result(result: std::result::Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_panic() {
            tracing::warn!(%error, "active health probe task panicked");
        } else if !error.is_cancelled() {
            tracing::warn!(%error, "active health probe task failed to join");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use rginx_core::{
        ActiveHealthCheck, ConfigSnapshot, RuntimeSettings, Server, Upstream, UpstreamPeer,
        UpstreamTls, VirtualHost,
    };
    use rginx_http::SharedState;

    use super::collect_probe_targets;

    #[tokio::test]
    async fn collect_probe_targets_only_includes_enabled_upstreams() {
        let healthy = Arc::new(Upstream::new(
            "healthy".to_string(),
            vec![peer("http://127.0.0.1:9000")],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            Some(ActiveHealthCheck {
                path: "/healthz".to_string(),
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(2),
                healthy_successes_required: 2,
            }),
        ));
        let passive_only = Arc::new(Upstream::new(
            "passive-only".to_string(),
            vec![peer("http://127.0.0.1:9010")],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            None,
        ));

        let snapshot = ConfigSnapshot {
            runtime: RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            default_vhost: VirtualHost {
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            routes: Vec::new(),
            upstreams: HashMap::from([
                ("healthy".to_string(), healthy),
                ("passive-only".to_string(), passive_only),
            ]),
        };

        let shared = SharedState::from_config(snapshot).expect("shared state should build");
        let targets = collect_probe_targets(shared.snapshot().await);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].key.upstream_name, "healthy");
        assert_eq!(targets[0].key.peer_url, "http://127.0.0.1:9000");
        assert_eq!(targets[0].health_check.path, "/healthz");
    }

    fn peer(url: &str) -> UpstreamPeer {
        let (scheme, authority) = url.split_once("://").expect("peer URL should include a scheme");
        UpstreamPeer {
            url: url.to_string(),
            scheme: scheme.to_string(),
            authority: authority.to_string(),
        }
    }
}
