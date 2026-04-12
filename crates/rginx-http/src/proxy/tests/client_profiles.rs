use super::*;

#[test]
fn upstream_request_version_follows_upstream_protocol() {
    assert_eq!(upstream_request_version(UpstreamProtocol::Auto), Version::HTTP_11);
    assert_eq!(upstream_request_version(UpstreamProtocol::Http1), Version::HTTP_11);
    assert_eq!(upstream_request_version(UpstreamProtocol::Http2), Version::HTTP_2);
    assert_eq!(upstream_request_version(UpstreamProtocol::Http3), Version::HTTP_3);
}

#[test]
fn load_custom_ca_store_accepts_pem_files() {
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let path = temp_dir.path().join("custom-ca.pem");
    std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

    let store = load_custom_ca_store(&path).expect("PEM CA should load");
    assert!(!store.is_empty());
}

#[test]
fn proxy_clients_can_select_insecure_and_custom_ca_modes() {
    let insecure = Upstream::new(
        "insecure".to_string(),
        vec![UpstreamPeer {
            url: "https://localhost:9443".to_string(),
            scheme: "https".to_string(),
            authority: "localhost:9443".to_string(),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::Insecure,
        upstream_settings(UpstreamProtocol::Auto),
    );

    let temp_dir = TempDir::new().expect("temp dir should be created");
    let path = temp_dir.path().join("custom-ca.pem");
    std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

    let custom = Upstream::new(
        "custom".to_string(),
        vec![UpstreamPeer {
            url: "https://localhost:9443".to_string(),
            scheme: "https".to_string(),
            authority: "localhost:9443".to_string(),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::CustomCa { ca_cert_path: path.clone() },
        upstream_settings(UpstreamProtocol::Auto),
    );

    let snapshot = snapshot_with_upstreams([
        ("insecure".to_string(), Arc::new(insecure)),
        ("custom".to_string(), Arc::new(custom)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert!(clients.for_upstream(snapshot.upstreams["insecure"].as_ref()).is_ok());
    assert!(clients.for_upstream(snapshot.upstreams["custom"].as_ref()).is_ok());
}

#[test]
fn proxy_clients_cache_distinguishes_server_name_override() {
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let path = temp_dir.path().join("custom-ca-override.pem");
    std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

    let peer = UpstreamPeer {
        url: "https://127.0.0.1:9443".to_string(),
        scheme: "https".to_string(),
        authority: "127.0.0.1:9443".to_string(),
        weight: 1,
        backup: false,
    };
    let first = Upstream::new(
        "first".to_string(),
        vec![peer.clone()],
        UpstreamTls::CustomCa { ca_cert_path: path.clone() },
        UpstreamSettings {
            server_name_override: Some("api-a.internal".to_string()),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let second = Upstream::new(
        "second".to_string(),
        vec![peer.clone()],
        UpstreamTls::CustomCa { ca_cert_path: path.clone() },
        UpstreamSettings {
            server_name_override: Some("api-b.internal".to_string()),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let duplicate = Upstream::new(
        "duplicate".to_string(),
        vec![peer],
        UpstreamTls::CustomCa { ca_cert_path: path.clone() },
        UpstreamSettings {
            server_name_override: Some("api-a.internal".to_string()),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );

    let snapshot = snapshot_with_upstreams([
        ("first".to_string(), Arc::new(first)),
        ("second".to_string(), Arc::new(second)),
        ("duplicate".to_string(), Arc::new(duplicate)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert_eq!(clients.cached_client_count(), 2);
    assert!(clients.for_upstream(snapshot.upstreams["first"].as_ref()).is_ok());
    assert!(clients.for_upstream(snapshot.upstreams["second"].as_ref()).is_ok());
    assert!(clients.for_upstream(snapshot.upstreams["duplicate"].as_ref()).is_ok());
}

#[test]
fn proxy_clients_cache_distinguishes_server_name_toggle() {
    let peer = UpstreamPeer {
        url: "https://127.0.0.1:9443".to_string(),
        scheme: "https".to_string(),
        authority: "127.0.0.1:9443".to_string(),
        weight: 1,
        backup: false,
    };
    let default_sni = Upstream::new(
        "default-sni".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );
    let no_sni = Upstream::new(
        "no-sni".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        UpstreamSettings { server_name: false, ..upstream_settings(UpstreamProtocol::Auto) },
    );
    let duplicate = Upstream::new(
        "duplicate".to_string(),
        vec![peer],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );

    let snapshot = snapshot_with_upstreams([
        ("default-sni".to_string(), Arc::new(default_sni)),
        ("no-sni".to_string(), Arc::new(no_sni)),
        ("duplicate".to_string(), Arc::new(duplicate)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert_eq!(clients.cached_client_count(), 2);
}

#[test]
fn proxy_clients_cache_distinguishes_upstream_protocol() {
    let peer = UpstreamPeer {
        url: "https://127.0.0.1:9443".to_string(),
        scheme: "https".to_string(),
        authority: "127.0.0.1:9443".to_string(),
        weight: 1,
        backup: false,
    };
    let auto = Upstream::new(
        "auto".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );
    let http1 = Upstream::new(
        "http1".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Http1),
    );
    let http2 = Upstream::new(
        "http2".to_string(),
        vec![peer],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Http2),
    );
    let http3 = Upstream::new(
        "http3".to_string(),
        vec![UpstreamPeer {
            url: "https://127.0.0.1:9444".to_string(),
            scheme: "https".to_string(),
            authority: "127.0.0.1:9444".to_string(),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Http3),
    );

    let snapshot = snapshot_with_upstreams([
        ("auto".to_string(), Arc::new(auto)),
        ("http1".to_string(), Arc::new(http1)),
        ("http2".to_string(), Arc::new(http2)),
        ("http3".to_string(), Arc::new(http3)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert_eq!(clients.cached_client_count(), 4);
}

#[test]
fn proxy_clients_cache_distinguishes_tls_versions() {
    let peer = UpstreamPeer {
        url: "https://127.0.0.1:9443".to_string(),
        scheme: "https".to_string(),
        authority: "127.0.0.1:9443".to_string(),
        weight: 1,
        backup: false,
    };
    let tls12 = Upstream::new(
        "tls12".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            tls_versions: Some(vec![TlsVersion::Tls12]),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let tls13 = Upstream::new(
        "tls13".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            tls_versions: Some(vec![TlsVersion::Tls13]),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let duplicate = Upstream::new(
        "duplicate".to_string(),
        vec![peer],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            tls_versions: Some(vec![TlsVersion::Tls12]),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );

    let snapshot = snapshot_with_upstreams([
        ("tls12".to_string(), Arc::new(tls12)),
        ("tls13".to_string(), Arc::new(tls13)),
        ("duplicate".to_string(), Arc::new(duplicate)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert_eq!(clients.cached_client_count(), 2);
}

#[test]
fn proxy_clients_cache_distinguishes_client_identity() {
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let dir = temp_dir.path().to_path_buf();
    let first_cert = dir.join("first.crt");
    let first_key = dir.join("first.key");
    let second_cert = dir.join("second.crt");
    let second_key = dir.join("second.key");
    write_test_identity(&first_cert, &first_key);
    write_test_identity(&second_cert, &second_key);

    let peer = UpstreamPeer {
        url: "https://127.0.0.1:9443".to_string(),
        scheme: "https".to_string(),
        authority: "127.0.0.1:9443".to_string(),
        weight: 1,
        backup: false,
    };
    let first = Upstream::new(
        "first".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            client_identity: Some(ClientIdentity {
                cert_path: first_cert.clone(),
                key_path: first_key.clone(),
            }),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let second = Upstream::new(
        "second".to_string(),
        vec![peer.clone()],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            client_identity: Some(ClientIdentity {
                cert_path: second_cert.clone(),
                key_path: second_key.clone(),
            }),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let duplicate = Upstream::new(
        "duplicate".to_string(),
        vec![peer],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            client_identity: Some(ClientIdentity {
                cert_path: first_cert.clone(),
                key_path: first_key.clone(),
            }),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );

    let snapshot = snapshot_with_upstreams([
        ("first".to_string(), Arc::new(first)),
        ("second".to_string(), Arc::new(second)),
        ("duplicate".to_string(), Arc::new(duplicate)),
    ]);

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    assert_eq!(clients.cached_client_count(), 2);
}

#[tokio::test]
async fn wait_for_upstream_stage_times_out() {
    let timeout = Duration::from_millis(25);

    let error = wait_for_upstream_stage(timeout, "backend", "request", async {
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await
    .expect_err("slow future should time out");

    assert!(matches!(error, Error::Server(message) if message.contains("timed out")));
}
