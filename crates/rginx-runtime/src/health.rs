use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rginx_core::{ActiveHealthCheck, Upstream, UpstreamPeer};
use rginx_http::SharedState;
use rginx_http::proxy::{ProxyClients, probe_upstream_peer};
use rginx_http::state::ActiveState;
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
            next_due.entry(target.key.clone()).or_insert_with(|| {
                initial_probe_due_at(now, &target.key, target.health_check.interval)
            });
        }

        let due_targets = targets
            .into_iter()
            .filter(|target| next_due.get(&target.key).is_some_and(|due_at| *due_at <= now))
            .collect::<Vec<_>>();

        if !due_targets.is_empty() {
            let mut probes = JoinSet::new();

            for target in due_targets {
                next_due.insert(target.key.clone(), now + target.health_check.interval);
                probes.spawn(async move {
                    probe_upstream_peer(target.clients, target.upstream, target.peer).await;
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

fn initial_probe_due_at(now: Instant, key: &ProbeKey, interval: std::time::Duration) -> Instant {
    now + initial_probe_delay(key, interval)
}

fn initial_probe_delay(key: &ProbeKey, interval: std::time::Duration) -> std::time::Duration {
    let interval_nanos = interval.as_nanos();
    if interval_nanos == 0 {
        return std::time::Duration::ZERO;
    }

    let jitter_nanos = stable_probe_hash(key) % interval_nanos;
    let jitter_nanos = jitter_nanos.min(u128::from(u64::MAX)) as u64;
    std::time::Duration::from_nanos(jitter_nanos)
}

fn stable_probe_hash(key: &ProbeKey) -> u128 {
    const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    key.upstream_name
        .bytes()
        .chain(std::iter::once(0))
        .chain(key.peer_url.bytes())
        .fold(FNV_OFFSET, |hash, byte| (hash ^ u128::from(byte)).wrapping_mul(FNV_PRIME))
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
mod tests;
