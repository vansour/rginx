use super::*;

impl PeerHealthPolicy {
    pub(super) fn from_upstream(upstream: &Upstream) -> Self {
        Self {
            unhealthy_after_failures: upstream.unhealthy_after_failures,
            cooldown: upstream.unhealthy_cooldown,
            active_health_enabled: upstream.active_health_check.is_some(),
        }
    }
}
