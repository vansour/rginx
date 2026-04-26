use super::*;

use super::guards::ActivePeerGuard;

impl PeerHealthRegistry {
    pub(crate) fn from_config(config: &ConfigSnapshot) -> Self {
        Self::from_config_with_notifier(config, None)
    }

    pub(crate) fn from_config_with_notifier(
        config: &ConfigSnapshot,
        notifier: Option<HealthChangeNotifier>,
    ) -> Self {
        let policies = config
            .upstreams
            .iter()
            .map(|(upstream_name, upstream)| {
                (upstream_name.clone(), PeerHealthPolicy::from_upstream(upstream.as_ref()))
            })
            .collect::<HashMap<_, _>>();
        let peers = config
            .upstreams
            .iter()
            .map(|(upstream_name, upstream)| {
                let peers = upstream
                    .peers
                    .iter()
                    .map(|peer| (peer.url.clone(), Arc::new(PeerHealth::default())))
                    .collect::<HashMap<_, _>>();
                (upstream_name.clone(), peers)
            })
            .collect::<HashMap<_, _>>();
        let endpoint_peers = config
            .upstreams
            .keys()
            .map(|upstream_name| (upstream_name.clone(), HashMap::new()))
            .collect::<HashMap<_, _>>();

        Self {
            policies: Arc::new(policies),
            peers: Arc::new(peers),
            endpoint_peers: Arc::new(Mutex::new(endpoint_peers)),
            notifier,
        }
    }

    pub(crate) fn record_success(&self, upstream_name: &str, peer_url: &str) -> bool {
        if let Some(health) = self.get_health(upstream_name, peer_url) {
            let recovered = health.record_success();
            self.notify_change(upstream_name);
            return recovered;
        }

        false
    }

    pub(crate) fn record_failure(&self, upstream_name: &str, peer_url: &str) -> PeerFailureStatus {
        let Some(policy) = self.policies.get(upstream_name).copied() else {
            return PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false };
        };

        self.get_health(upstream_name, peer_url)
            .map(|health| {
                let status = health.record_failure(policy);
                self.notify_change(upstream_name);
                status
            })
            .unwrap_or(PeerFailureStatus { consecutive_failures: 0, entered_cooldown: false })
    }

    pub(super) fn is_available(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get_health(upstream_name, peer_url).is_none_or(|health| health.is_available())
    }

    pub(crate) fn record_active_success(
        &self,
        upstream_name: &str,
        peer_url: &str,
        healthy_successes_required: u32,
    ) -> ActiveProbeStatus {
        self.get_health(upstream_name, peer_url)
            .map(|health| {
                let status = health.record_active_success(healthy_successes_required);
                self.notify_change(upstream_name);
                status
            })
            .unwrap_or(ActiveProbeStatus {
                healthy: true,
                recovered: false,
                consecutive_successes: 0,
            })
    }

    pub(crate) fn record_active_failure(&self, upstream_name: &str, peer_url: &str) -> bool {
        self.get_health(upstream_name, peer_url).is_some_and(|health| {
            let changed = health.record_active_failure();
            self.notify_change(upstream_name);
            changed
        })
    }

    pub(crate) fn track_active_request(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> ActivePeerGuard {
        let peer = self.get_health(upstream_name, peer_url);
        if let Some(ref peer) = peer {
            let transitioned_from_idle = peer.increment_active_requests();
            if transitioned_from_idle {
                self.notify_change(upstream_name);
            }
        }

        ActivePeerGuard {
            peer,
            notifier: self.notifier.clone(),
            upstream_name: upstream_name.to_string(),
        }
    }

    pub(crate) fn active_requests(&self, upstream_name: &str, peer_url: &str) -> u64 {
        self.get_health(upstream_name, peer_url).map(|health| health.active_requests()).unwrap_or(0)
    }

    pub(super) fn get_health(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<Arc<PeerHealth>> {
        self.get_endpoint(upstream_name, peer_url)
            .or_else(|| self.get_logical_peer(upstream_name, peer_url))
    }

    pub(super) fn get_logical_peer(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<Arc<PeerHealth>> {
        self.peers
            .get(upstream_name)
            .and_then(|upstream_peers| upstream_peers.get(peer_url))
            .cloned()
    }

    pub(super) fn get_endpoint(
        &self,
        upstream_name: &str,
        peer_url: &str,
    ) -> Option<Arc<PeerHealth>> {
        self.endpoint_peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(upstream_name)
            .and_then(|upstream_peers| upstream_peers.get(peer_url))
            .cloned()
    }

    pub(super) fn ensure_endpoint(
        &self,
        upstream_name: &str,
        endpoint_key: &str,
        logical_peer_url: &str,
    ) -> Arc<PeerHealth> {
        let mut endpoints =
            self.endpoint_peers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        endpoints
            .entry(upstream_name.to_string())
            .or_default()
            .entry(endpoint_key.to_string())
            .or_insert_with(|| {
                if endpoint_key == logical_peer_url {
                    self.get_logical_peer(upstream_name, logical_peer_url)
                        .unwrap_or_else(|| Arc::new(PeerHealth::default()))
                } else {
                    Arc::new(PeerHealth::default())
                }
            })
            .clone()
    }

    pub(super) fn notify_change(&self, upstream_name: &str) {
        if let Some(notifier) = &self.notifier {
            notifier(upstream_name);
        }
    }
}

impl PeerHealth {
    pub(super) fn is_available(&self) -> bool {
        let state = lock_peer_health(&self.state);
        let passive_available =
            state.passive.unhealthy_until.is_none_or(|until| until <= Instant::now());
        passive_available && !state.active.unhealthy
    }

    fn record_success(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let recovered = state.passive.pending_recovery;
        state.passive = PassiveHealthState::default();
        recovered
    }

    fn record_failure(&self, policy: PeerHealthPolicy) -> PeerFailureStatus {
        let mut state = lock_peer_health(&self.state);
        let now = Instant::now();

        if state.passive.unhealthy_until.is_some_and(|until| until <= now) {
            state.passive.unhealthy_until = None;
            state.passive.consecutive_failures = 0;
        }

        let already_in_cooldown = state.passive.unhealthy_until.is_some_and(|until| until > now);
        state.passive.consecutive_failures += 1;
        let entered_cooldown = !already_in_cooldown
            && state.passive.consecutive_failures >= policy.unhealthy_after_failures;
        if entered_cooldown {
            state.passive.unhealthy_until = Some(now + policy.cooldown);
            state.passive.pending_recovery = true;
        } else if already_in_cooldown {
            state.passive.unhealthy_until = Some(now + policy.cooldown);
        }

        PeerFailureStatus {
            consecutive_failures: state.passive.consecutive_failures,
            entered_cooldown,
        }
    }

    fn record_active_success(&self, healthy_successes_required: u32) -> ActiveProbeStatus {
        let mut state = lock_peer_health(&self.state);
        if !state.active.unhealthy {
            return ActiveProbeStatus { healthy: true, recovered: false, consecutive_successes: 0 };
        }

        state.active.consecutive_successes += 1;
        let consecutive_successes = state.active.consecutive_successes;
        let recovered = consecutive_successes >= healthy_successes_required;
        if recovered {
            state.active = ActiveHealthState::default();
        }

        ActiveProbeStatus { healthy: recovered, recovered, consecutive_successes }
    }

    fn record_active_failure(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let was_healthy = !state.active.unhealthy;
        state.active.unhealthy = true;
        state.active.consecutive_successes = 0;
        was_healthy
    }

    fn increment_active_requests(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let transitioned_from_idle = state.active_requests == 0;
        state.active_requests += 1;
        transitioned_from_idle
    }

    pub(super) fn decrement_active_requests(&self) -> bool {
        let mut state = lock_peer_health(&self.state);
        let was_active = state.active_requests > 0;
        state.active_requests = state.active_requests.saturating_sub(1);
        was_active && state.active_requests == 0
    }

    pub(super) fn active_requests(&self) -> u64 {
        lock_peer_health(&self.state).active_requests
    }
}

pub(super) fn lock_peer_health(
    state: &Mutex<PeerHealthState>,
) -> std::sync::MutexGuard<'_, PeerHealthState> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
