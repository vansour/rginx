use std::collections::HashMap;
use std::time::Duration;

use super::*;

fn listener(
    name: &str,
    tls: bool,
    tls_termination_enabled: bool,
    default_certificate: Option<&str>,
) -> Listener {
    Listener {
        id: format!("listener:{name}"),
        name: name.to_string(),
        server: rginx_core::Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            server_header: rginx_core::default_server_header(),
            default_certificate: default_certificate.map(str::to_string),
            trusted_proxies: Vec::new(),
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            request_body_read_timeout: None,
            response_write_timeout: None,
            access_log_format: None,
            tls: tls.then_some(rginx_core::ServerTls {
                cert_path: "/tmp/listener.crt".into(),
                key_path: "/tmp/listener.key".into(),
                additional_certificates: Vec::new(),
                versions: None,
                cipher_suites: None,
                key_exchange_groups: None,
                alpn_protocols: None,
                ocsp_staple_path: None,
                ocsp: rginx_core::OcspConfig::default(),
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
        },
        tls_termination_enabled,
        proxy_protocol_enabled: false,
        http3: None,
    }
}

fn vhost(id: &str, server_names: &[&str], tls: bool) -> rginx_core::VirtualHost {
    rginx_core::VirtualHost {
        id: id.to_string(),
        server_names: server_names.iter().map(|name| (*name).to_string()).collect(),
        routes: Vec::new(),
        tls: tls.then_some(rginx_core::VirtualHostTls {
            cert_path: format!("/tmp/{id}.crt").into(),
            key_path: format!("/tmp/{id}.key").into(),
            additional_certificates: Vec::new(),
            ocsp_staple_path: None,
            ocsp: rginx_core::OcspConfig::default(),
        }),
    }
}

fn certificates(scopes: &[&str]) -> Vec<TlsCertificateStatusSnapshot> {
    scopes
        .iter()
        .map(|scope| TlsCertificateStatusSnapshot {
            scope: (*scope).to_string(),
            cert_path: format!("/tmp/{scope}.crt").into(),
            server_names: Vec::new(),
            subject: None,
            issuer: None,
            serial_number: None,
            san_dns_names: Vec::new(),
            fingerprint_sha256: Some(format!("fp-{scope}")),
            subject_key_identifier: None,
            authority_key_identifier: None,
            is_ca: None,
            path_len_constraint: None,
            key_usage: None,
            extended_key_usage: Vec::new(),
            not_before_unix_ms: None,
            not_after_unix_ms: None,
            expires_in_days: None,
            chain_length: 0,
            chain_subjects: Vec::new(),
            chain_diagnostics: Vec::new(),
            selected_as_default_for_listeners: Vec::new(),
            ocsp_staple_configured: false,
            additional_certificate_count: 0,
        })
        .collect()
}

#[test]
fn listener_certificate_is_default_when_no_explicit_default_is_configured() {
    let config = ConfigSnapshot {
        runtime: rginx_core::RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![listener("default", true, true, None)],
        default_vhost: vhost("server", &["default.example.com"], false),
        vhosts: vec![vhost("servers[0]", &["api.example.com"], true)],
        upstreams: HashMap::new(),
    };

    let (vhost_bindings, sni_bindings, _conflicts, default_bindings) =
        tls_binding_snapshots(&config, &certificates(&["listener:default", "vhost:servers[0]"]));

    assert!(default_bindings.is_empty());
    assert!(
        vhost_bindings
            .iter()
            .any(|binding| { binding.vhost_id == "server" && binding.default_selected })
    );
    assert!(
        vhost_bindings
            .iter()
            .any(|binding| { binding.vhost_id == "servers[0]" && !binding.default_selected })
    );
    assert!(sni_bindings.iter().any(|binding| {
        binding.server_name == "default.example.com" && binding.default_selected
    }));
    assert!(
        sni_bindings.iter().any(|binding| {
            binding.server_name == "api.example.com" && !binding.default_selected
        })
    );
}

#[test]
fn single_named_vhost_certificate_becomes_implicit_default_without_listener_tls() {
    let config = ConfigSnapshot {
        runtime: rginx_core::RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![listener("default", false, true, None)],
        default_vhost: vhost("server", &[], false),
        vhosts: vec![vhost("servers[0]", &["api.example.com"], true)],
        upstreams: HashMap::new(),
    };

    let (_vhost_bindings, sni_bindings, _conflicts, default_bindings) =
        tls_binding_snapshots(&config, &certificates(&["vhost:servers[0]"]));

    assert_eq!(default_bindings.len(), 1);
    assert_eq!(default_bindings[0].server_name, "api.example.com");
    assert!(sni_bindings[0].default_selected);
}
