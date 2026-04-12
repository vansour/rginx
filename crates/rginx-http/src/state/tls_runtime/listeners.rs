use super::*;

pub(super) fn tls_listener_status_snapshots(
    config: &ConfigSnapshot,
) -> Vec<TlsListenerStatusSnapshot> {
    config
        .listeners
        .iter()
        .map(|listener| {
            let sni_names = tls_listener_sni_names(config, listener.tls_enabled());
            let tls = listener.server.tls.as_ref();
            let http3 = listener.http3.as_ref();
            TlsListenerStatusSnapshot {
                listener_id: listener.id.clone(),
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                tls_enabled: listener.tls_enabled(),
                http3_enabled: http3.is_some(),
                http3_listen_addr: http3.map(|http3| http3.listen_addr),
                default_certificate: listener.server.default_certificate.clone(),
                versions: tls.and_then(|tls| {
                    tls.versions.as_ref().map(|versions| {
                        versions
                            .iter()
                            .map(|version| tls_version_label(*version).to_string())
                            .collect()
                    })
                }),
                alpn_protocols: tls
                    .and_then(|tls| tls.alpn_protocols.clone())
                    .unwrap_or_else(|| vec!["h2".to_string(), "http/1.1".to_string()]),
                http3_versions: http3
                    .map(|_| vec![tls_version_label(rginx_core::TlsVersion::Tls13).to_string()])
                    .unwrap_or_default(),
                http3_alpn_protocols: http3.map(|_| vec!["h3".to_string()]).unwrap_or_default(),
                session_resumption_enabled: tls.map(|tls| tls.session_resumption != Some(false)),
                session_tickets_enabled: tls.map(|tls| {
                    tls.session_resumption != Some(false) && tls.session_tickets != Some(false)
                }),
                session_cache_size: tls.map(|tls| {
                    if tls.session_resumption == Some(false) {
                        0
                    } else {
                        tls.session_cache_size.unwrap_or(256)
                    }
                }),
                session_ticket_count: tls.map(|tls| {
                    if tls.session_resumption == Some(false) || tls.session_tickets == Some(false) {
                        0
                    } else {
                        tls.session_ticket_count.unwrap_or(2)
                    }
                }),
                client_auth_mode: tls.and_then(|tls| {
                    tls.client_auth.as_ref().map(|client_auth| match client_auth.mode {
                        rginx_core::ServerClientAuthMode::Optional => "optional".to_string(),
                        rginx_core::ServerClientAuthMode::Required => "required".to_string(),
                    })
                }),
                client_auth_verify_depth: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.verify_depth),
                client_auth_crl_configured: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.crl_path.as_ref())
                    .is_some(),
                sni_names,
            }
        })
        .collect()
}

fn tls_listener_sni_names(config: &ConfigSnapshot, listener_has_tls: bool) -> Vec<String> {
    if !listener_has_tls && !config.vhosts.iter().any(|vhost| vhost.tls.is_some()) {
        return Vec::new();
    }

    let mut names = config.default_vhost.server_names.clone();
    for vhost in &config.vhosts {
        if vhost.tls.is_some() {
            names.extend(vhost.server_names.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn tls_version_label(version: rginx_core::TlsVersion) -> &'static str {
    match version {
        rginx_core::TlsVersion::Tls12 => "TLS1.2",
        rginx_core::TlsVersion::Tls13 => "TLS1.3",
    }
}
