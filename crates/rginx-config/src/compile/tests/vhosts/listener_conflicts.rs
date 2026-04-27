use super::*;

#[test]
fn compile_rejects_conflicting_shared_vhost_listener_flags() {
    let cases = vec![
        (
            vec![
                listen_vhost("plain.example.com", "127.0.0.1:8443", false, None),
                listen_vhost("tls.example.com", "127.0.0.1:8443 ssl", true, None),
            ],
            "mixes ssl",
        ),
        (
            vec![
                listen_vhost("proxy.example.com", "127.0.0.1:8443 ssl proxy_protocol", true, None),
                listen_vhost("direct.example.com", "127.0.0.1:8443 ssl", true, None),
            ],
            "mixes proxy_protocol",
        ),
        (
            vec![
                listen_vhost(
                    "h3a.example.com",
                    "127.0.0.1:8443 ssl http2 http3",
                    true,
                    Some(Http3Config {
                        alt_svc_max_age_secs: Some(7200),
                        ..Http3Config::default()
                    }),
                ),
                listen_vhost(
                    "h3b.example.com",
                    "127.0.0.1:8443 ssl http2 http3",
                    true,
                    Some(Http3Config {
                        alt_svc_max_age_secs: Some(3600),
                        ..Http3Config::default()
                    }),
                ),
            ],
            "must use consistent http3 settings",
        ),
    ];

    for (servers, expected) in cases {
        let config = Config {
            runtime: RuntimeConfig {
                shutdown_timeout_secs: 10,
                worker_threads: None,
                accept_workers: None,
            },
            listeners: Vec::new(),
            server: server_defaults(None),
            upstreams: Vec::new(),
            locations: Vec::new(),
            servers,
        };

        let error = compile(config).expect_err("conflicting vhost listen flags should fail");
        assert!(error.to_string().contains(expected));
    }
}

fn listen_vhost(
    server_name: &str,
    listen: &str,
    tls: bool,
    http3: Option<Http3Config>,
) -> VirtualHostConfig {
    VirtualHostConfig {
        listen: vec![listen.to_string()],
        server_names: vec![server_name.to_string()],
        upstreams: Vec::new(),
        locations: vec![return_location("ok\n")],
        tls: tls.then(|| crate::model::VirtualHostTlsConfig {
            cert_path: format!("{server_name}.crt"),
            key_path: format!("{server_name}.key"),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
        http3,
    }
}
