use super::super::*;

pub(super) fn traffic_topology_changed(previous: &ConfigSnapshot, next: &ConfigSnapshot) -> bool {
    let listener_ids = |config: &ConfigSnapshot| {
        config
            .listeners
            .iter()
            .map(|listener| listener.id.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let vhost_ids = |config: &ConfigSnapshot| {
        std::iter::once(&config.default_vhost)
            .chain(config.vhosts.iter())
            .map(|vhost| vhost.id.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let route_ids = |config: &ConfigSnapshot| {
        std::iter::once(&config.default_vhost)
            .chain(config.vhosts.iter())
            .flat_map(|vhost| vhost.routes.iter().map(|route| route.id.clone()))
            .collect::<std::collections::BTreeSet<_>>()
    };

    listener_ids(previous) != listener_ids(next)
        || vhost_ids(previous) != vhost_ids(next)
        || route_ids(previous) != route_ids(next)
}

pub(super) fn upstream_topology_changed(previous: &ConfigSnapshot, next: &ConfigSnapshot) -> bool {
    previous.upstreams.keys().collect::<std::collections::BTreeSet<_>>()
        != next.upstreams.keys().collect::<std::collections::BTreeSet<_>>()
}
