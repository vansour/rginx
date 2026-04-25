use rginx_core::{ConfigSnapshot, Error, Result};

const RELOADABLE_FIELDS: [&str; 10] = [
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
];

const RESTART_REQUIRED_FIELDS: [&str; 6] = [
    "listen",
    "server.http3.listen",
    "listeners[].listen",
    "listeners[].http3.listen",
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
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use rginx_core::{ConfigSnapshot, Listener, RuntimeSettings, Server, VirtualHost};

    use super::{
        ConfigTransitionKind, config_transition_boundary, plan_config_transition,
        validate_config_transition,
    };

    fn snapshot(listen: &str) -> ConfigSnapshot {
        let server = Server {
            listen_addr: listen.parse().unwrap(),
            server_header: rginx_core::default_server_header(),
            default_certificate: None,
            trusted_proxies: Vec::new(),
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            request_body_read_timeout: None,
            response_write_timeout: None,
            access_log_format: None,
            tls: None,
        };
        ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(10),
                worker_threads: None,
                accept_workers: 1,
            },
            listeners: vec![Listener {
                id: "default".to_string(),
                name: "default".to_string(),
                server,
                tls_termination_enabled: false,
                proxy_protocol_enabled: false,
                http3: None,
            }],
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::new(),
        }
    }

    #[test]
    fn boundary_lists_are_stable() {
        let boundary = config_transition_boundary();
        assert_eq!(
            boundary.reloadable_fields,
            vec![
                "server.tls".to_string(),
                "server.http3.advertise_alt_svc".to_string(),
                "server.http3.alt_svc_max_age_secs".to_string(),
                "listeners[].tls".to_string(),
                "listeners[].http3.advertise_alt_svc".to_string(),
                "listeners[].http3.alt_svc_max_age_secs".to_string(),
                "servers[].tls".to_string(),
                "upstreams[].tls".to_string(),
                "upstreams[].server_name".to_string(),
                "upstreams[].server_name_override".to_string(),
            ]
        );
        assert_eq!(
            boundary.restart_required_fields,
            vec![
                "listen".to_string(),
                "server.http3.listen".to_string(),
                "listeners[].listen".to_string(),
                "listeners[].http3.listen".to_string(),
                "runtime.worker_threads".to_string(),
                "runtime.accept_workers".to_string(),
            ]
        );
    }

    #[test]
    fn planner_returns_hot_reload_for_unchanged_startup_boundary() {
        let current = snapshot("127.0.0.1:8080");
        let next = snapshot("127.0.0.1:8080");

        let plan = plan_config_transition(&current, &next);
        assert_eq!(plan.kind, ConfigTransitionKind::HotReload);
        assert!(plan.changed_restart_required_fields.is_empty());
        validate_config_transition(&current, &next).expect("unchanged boundary should hot reload");
    }

    #[test]
    fn planner_reports_restart_required_changes() {
        let mut current = snapshot("127.0.0.1:8080");
        current.runtime.worker_threads = Some(2);
        let mut next = snapshot("127.0.0.1:9090");
        next.runtime.worker_threads = Some(4);
        next.runtime.accept_workers = 2;

        let plan = plan_config_transition(&current, &next);
        assert_eq!(plan.kind, ConfigTransitionKind::RestartRequired);
        assert!(
            plan.changed_restart_required_fields
                .iter()
                .any(|change| change.contains("default.listen 127.0.0.1:8080 -> 127.0.0.1:9090"))
        );
        assert!(
            plan.changed_restart_required_fields
                .iter()
                .any(|change| change.contains("runtime.worker_threads Some(2) -> Some(4)"))
        );
        assert!(
            plan.changed_restart_required_fields
                .iter()
                .any(|change| change.contains("runtime.accept_workers 1 -> 2"))
        );
        let error = validate_config_transition(&current, &next)
            .expect_err("startup boundary change should require restart");
        assert!(error.to_string().contains("reload requires restart"));
    }

    #[test]
    fn planner_allows_listener_add_remove_when_existing_listener_addresses_stay_stable() {
        let current = snapshot("127.0.0.1:8080");
        let mut next = snapshot("127.0.0.1:8080");
        next.listeners = vec![
            current.listeners[0].clone(),
            rginx_core::Listener {
                id: "listener:https".to_string(),
                name: "https".to_string(),
                server: rginx_core::Server {
                    listen_addr: "127.0.0.1:8443".parse().unwrap(),
                    server_header: rginx_core::default_server_header(),
                    ..current.listeners[0].server.clone()
                },
                tls_termination_enabled: false,
                proxy_protocol_enabled: false,
                http3: None,
            },
        ];

        let plan = plan_config_transition(&current, &next);
        assert_eq!(plan.kind, ConfigTransitionKind::HotReload);
        assert!(plan.changed_restart_required_fields.is_empty());
        validate_config_transition(&current, &next)
            .expect("listener add/remove should stay within the hot-reload boundary");
    }

    #[test]
    fn planner_reports_restart_required_http3_listener_binding_changes() {
        let mut current = snapshot("127.0.0.1:8443");
        current.listeners[0].tls_termination_enabled = true;
        current.listeners[0].http3 = Some(rginx_core::ListenerHttp3 {
            listen_addr: "127.0.0.1:8443".parse().unwrap(),
            advertise_alt_svc: true,
            alt_svc_max_age: Duration::from_secs(3600),
            max_concurrent_streams: 128,
            stream_buffer_size: 64 * 1024,
            active_connection_id_limit: 2,
            retry: false,
            host_key_path: None,
            gso: false,
            early_data_enabled: false,
        });

        let mut next = snapshot("127.0.0.1:8443");
        next.listeners[0].tls_termination_enabled = true;
        next.listeners[0].http3 = Some(rginx_core::ListenerHttp3 {
            listen_addr: "127.0.0.1:9443".parse().unwrap(),
            advertise_alt_svc: true,
            alt_svc_max_age: Duration::from_secs(3600),
            max_concurrent_streams: 128,
            stream_buffer_size: 64 * 1024,
            active_connection_id_limit: 2,
            retry: false,
            host_key_path: None,
            gso: false,
            early_data_enabled: false,
        });

        let plan = plan_config_transition(&current, &next);
        assert_eq!(plan.kind, ConfigTransitionKind::RestartRequired);
        assert!(plan.changed_restart_required_fields.iter().any(|change| {
            change.contains("default.http3.listen 127.0.0.1:8443 -> 127.0.0.1:9443")
        }));
    }
}
