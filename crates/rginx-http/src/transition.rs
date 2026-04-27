use rginx_core::{ConfigSnapshot, Error, Result};

const RELOADABLE_FIELDS: [&str; 13] = [
    "server.tls",
    "server.http3.advertise_alt_svc",
    "server.http3.alt_svc_max_age_secs",
    "listeners[].tls",
    "listeners[].http3.advertise_alt_svc",
    "listeners[].http3.alt_svc_max_age_secs",
    "servers[].tls",
    "upstreams[].tls",
    "upstreams[].server_name",
    "upstreams[].server_name_override",
    "servers[].upstreams[].tls",
    "servers[].upstreams[].server_name",
    "servers[].upstreams[].server_name_override",
];

const RESTART_REQUIRED_FIELDS: [&str; 7] = [
    "listen",
    "server.http3.listen",
    "listeners[].listen",
    "listeners[].http3.listen",
    "servers[].listen",
    "runtime.worker_threads",
    "runtime.accept_workers",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigTransitionKind {
    HotReload,
    RestartRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigTransitionBoundary {
    pub reloadable_fields: Vec<String>,
    pub restart_required_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigTransitionPlan {
    pub kind: ConfigTransitionKind,
    pub boundary: ConfigTransitionBoundary,
    pub changed_restart_required_fields: Vec<String>,
}

impl ConfigTransitionBoundary {
    pub fn current() -> Self {
        Self {
            reloadable_fields: RELOADABLE_FIELDS.iter().map(|field| (*field).to_string()).collect(),
            restart_required_fields: RESTART_REQUIRED_FIELDS
                .iter()
                .map(|field| (*field).to_string())
                .collect(),
        }
    }
}

impl ConfigTransitionPlan {
    pub fn requires_restart(&self) -> bool {
        self.kind == ConfigTransitionKind::RestartRequired
    }

    pub fn restart_required_message(&self) -> Option<String> {
        self.requires_restart().then(|| {
            format!(
                "reload requires restart because these startup-boundary fields changed (restart-boundary: {}): {}",
                self.boundary.restart_required_fields.join(", "),
                self.changed_restart_required_fields.join("; ")
            )
        })
    }
}

pub fn config_transition_boundary() -> ConfigTransitionBoundary {
    ConfigTransitionBoundary::current()
}

pub fn plan_config_transition(
    current: &ConfigSnapshot,
    next: &ConfigSnapshot,
) -> ConfigTransitionPlan {
    let mut changes = Vec::new();

    let next_by_id = next
        .listeners
        .iter()
        .map(|listener| (listener.id.as_str(), listener))
        .collect::<std::collections::HashMap<_, _>>();

    for current_listener in &current.listeners {
        if let Some(next_listener) = next_by_id.get(current_listener.id.as_str()) {
            if current_listener.server.listen_addr != next_listener.server.listen_addr {
                changes.push(format!(
                    "{}.listen {} -> {}",
                    current_listener.id,
                    current_listener.server.listen_addr,
                    next_listener.server.listen_addr
                ));
            }

            let current_http3 = current_listener.http3.as_ref().map(|http3| http3.listen_addr);
            let next_http3 = next_listener_http3(Some(next_listener));
            if current_http3 != next_http3 {
                changes.push(format!(
                    "{}.http3.listen {} -> {}",
                    current_listener.id,
                    current_http3
                        .map(|listen_addr| listen_addr.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    next_http3
                        .map(|listen_addr| listen_addr.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ));
            }
        }
    }

    if current.runtime.worker_threads != next.runtime.worker_threads {
        changes.push(format!(
            "runtime.worker_threads {:?} -> {:?}",
            current.runtime.worker_threads, next.runtime.worker_threads
        ));
    }

    if current.runtime.accept_workers != next.runtime.accept_workers {
        changes.push(format!(
            "runtime.accept_workers {} -> {}",
            current.runtime.accept_workers, next.runtime.accept_workers
        ));
    }

    let boundary = config_transition_boundary();
    let kind = if changes.is_empty() {
        ConfigTransitionKind::HotReload
    } else {
        ConfigTransitionKind::RestartRequired
    };

    ConfigTransitionPlan { kind, boundary, changed_restart_required_fields: changes }
}

fn next_listener_http3(listener: Option<&rginx_core::Listener>) -> Option<std::net::SocketAddr> {
    listener.and_then(|listener| listener.http3.as_ref()).map(|http3| http3.listen_addr)
}

pub fn validate_config_transition(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    let plan = plan_config_transition(current, next);
    if let Some(message) = plan.restart_required_message() {
        return Err(Error::Config(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
