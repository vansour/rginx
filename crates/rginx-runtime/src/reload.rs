use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Error, Result};

use crate::state::RuntimeState;

pub async fn reload(state: &RuntimeState) -> Result<Arc<ConfigSnapshot>> {
    let next = rginx_config::load_and_compile(&state.config_path)?;
    let current = state.current_config().await;

    validate_reload(current.as_ref(), &next)?;
    state.http.replace(next).await
}

fn validate_reload(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    if current.server.listen_addr != next.server.listen_addr {
        return Err(Error::Config(format!(
            "reloading listen address from `{}` to `{}` is not supported; restart rginx instead",
            current.server.listen_addr, next.server.listen_addr
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use rginx_core::{RuntimeSettings, Server, VirtualHost};

    use super::{validate_reload, ConfigSnapshot};

    fn snapshot(listen: &str) -> ConfigSnapshot {
        ConfigSnapshot {
            runtime: RuntimeSettings { shutdown_timeout: Duration::from_secs(10) },
            server: Server {
                listen_addr: listen.parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
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
    fn reload_allows_unchanged_listener() {
        let current = snapshot("127.0.0.1:8080");
        let next = snapshot("127.0.0.1:8080");

        validate_reload(&current, &next).expect("reload should allow the same listener");
    }

    #[test]
    fn reload_rejects_listener_change() {
        let current = snapshot("127.0.0.1:8080");
        let next = snapshot("127.0.0.1:9090");

        let error = validate_reload(&current, &next).expect_err("reload should reject rebinding");
        assert!(error.to_string().contains("restart rginx"));
    }
}
