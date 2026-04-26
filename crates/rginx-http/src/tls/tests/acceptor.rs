use super::*;

#[test]
fn build_tls_acceptor_returns_none_for_plain_http() {
    let default_vhost = VirtualHost {
        id: "server".to_string(),
        server_names: Vec::new(),
        routes: Vec::new(),
        tls: None,
    };
    let vhosts: Vec<VirtualHost> = Vec::new();

    assert!(build_tls_acceptor(None, None, false, &default_vhost, &vhosts).unwrap().is_none());
}

#[test]
fn build_tls_acceptor_loads_valid_pem_files() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-{unique}"));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

    let server_tls = rginx_core::ServerTls {
        cert_path: cert_path.clone(),
        key_path: key_path.clone(),
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
    };
    let default_vhost = VirtualHost {
        id: "server".to_string(),
        server_names: vec!["localhost".to_string()],
        routes: Vec::new(),
        tls: None,
    };
    let vhosts: Vec<VirtualHost> = Vec::new();

    let acceptor = build_tls_acceptor(Some(&server_tls), None, true, &default_vhost, &vhosts)
        .expect("TLS acceptor should load");
    assert!(acceptor.is_some());
    assert_eq!(
        acceptor
            .expect("TLS acceptor should exist")
            .config()
            .alpn_protocols
            .iter()
            .map(|protocol| protocol.as_slice())
            .collect::<Vec<_>>(),
        vec![b"h2".as_slice(), b"http/1.1".as_slice()]
    );

    fs::remove_file(cert_path).expect("test cert should be removed");
    fs::remove_file(key_path).expect("test key should be removed");
    fs::remove_dir(temp_dir).expect("temp dir should be removed");
}

#[test]
fn build_tls_acceptor_respects_custom_alpn_protocols() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-alpn-{unique}"));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

    let tls = ServerTls {
        cert_path: cert_path.clone(),
        key_path: key_path.clone(),
        additional_certificates: Vec::new(),
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: Some(vec!["http/1.1".to_string()]),
        ocsp_staple_path: None,
        ocsp: rginx_core::OcspConfig::default(),
        session_resumption: None,
        session_tickets: None,
        session_cache_size: None,
        session_ticket_count: None,
        client_auth: None,
    };
    let default_vhost = VirtualHost {
        id: "server".to_string(),
        server_names: vec!["localhost".to_string()],
        routes: Vec::new(),
        tls: None,
    };

    let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
        .expect("TLS acceptor should load")
        .expect("TLS acceptor should exist");
    assert_eq!(
        acceptor
            .config()
            .alpn_protocols
            .iter()
            .map(|protocol| protocol.as_slice())
            .collect::<Vec<_>>(),
        vec![b"http/1.1".as_slice()]
    );

    fs::remove_file(cert_path).expect("test cert should be removed");
    fs::remove_file(key_path).expect("test key should be removed");
    fs::remove_dir(temp_dir).expect("temp dir should be removed");
}

#[test]
fn build_tls_acceptor_rejects_unknown_default_certificate_name() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-default-cert-{unique}"));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

    let vhosts = vec![VirtualHost {
        id: "servers[0]".to_string(),
        server_names: vec!["app.example.com".to_string()],
        routes: Vec::new(),
        tls: Some(VirtualHostTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            ocsp_staple_path: None,
            ocsp: rginx_core::OcspConfig::default(),
        }),
    }];
    let default_vhost = VirtualHost {
        id: "server".to_string(),
        server_names: Vec::new(),
        routes: Vec::new(),
        tls: None,
    };

    let error = match build_tls_acceptor(
        None,
        Some("missing.example.com"),
        true,
        &default_vhost,
        &vhosts,
    ) {
        Ok(_) => panic!("unknown default_certificate should be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("default_certificate `missing.example.com`"));

    fs::remove_file(cert_path).expect("test cert should be removed");
    fs::remove_file(key_path).expect("test key should be removed");
    fs::remove_dir(temp_dir).expect("temp dir should be removed");
}

#[test]
fn build_tls_acceptor_uses_single_vhost_cert_as_implicit_default() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-single-default-{unique}"));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

    let vhosts = vec![VirtualHost {
        id: "servers[0]".to_string(),
        server_names: vec!["app.example.com".to_string()],
        routes: Vec::new(),
        tls: Some(VirtualHostTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            ocsp_staple_path: None,
            ocsp: rginx_core::OcspConfig::default(),
        }),
    }];
    let default_vhost = VirtualHost {
        id: "server".to_string(),
        server_names: Vec::new(),
        routes: Vec::new(),
        tls: None,
    };

    let acceptor = build_tls_acceptor(None, None, true, &default_vhost, &vhosts)
        .expect("single vhost cert should become implicit default");
    assert!(acceptor.is_some());

    fs::remove_file(cert_path).expect("test cert should be removed");
    fs::remove_file(key_path).expect("test key should be removed");
    fs::remove_dir(temp_dir).expect("temp dir should be removed");
}
