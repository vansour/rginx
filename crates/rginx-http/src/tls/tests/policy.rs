use super::*;

#[test]
fn build_tls_acceptor_applies_custom_cipher_suites_and_groups() {
    let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-policy");
    let tls = ServerTls {
        cert_path: cert_path.clone(),
        key_path: key_path.clone(),
        additional_certificates: Vec::new(),
        versions: Some(vec![rginx_core::TlsVersion::Tls13]),
        cipher_suites: Some(vec![TlsCipherSuite::Tls13Aes128GcmSha256]),
        key_exchange_groups: Some(vec![TlsKeyExchangeGroup::Secp256r1]),
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

    let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
        .expect("TLS acceptor should load")
        .expect("TLS acceptor should exist");
    assert_eq!(
        acceptor.config().crypto_provider().cipher_suites[0].suite(),
        CipherSuite::TLS13_AES_128_GCM_SHA256
    );
    assert_eq!(acceptor.config().crypto_provider().kx_groups[0].name(), NamedGroup::secp256r1);

    remove_test_cert_pair(cert_path, key_path, temp_dir);
}

#[test]
fn build_tls_acceptor_disables_session_resumption_when_requested() {
    let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-resumption");
    let tls = ServerTls {
        cert_path: cert_path.clone(),
        key_path: key_path.clone(),
        additional_certificates: Vec::new(),
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: rginx_core::OcspConfig::default(),
        session_resumption: Some(false),
        session_tickets: Some(false),
        session_cache_size: Some(0),
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
    assert!(!acceptor.config().session_storage.can_cache());
    assert!(!acceptor.config().ticketer.enabled());
    assert_eq!(acceptor.config().send_tls13_tickets, 0);

    remove_test_cert_pair(cert_path, key_path, temp_dir);
}

#[test]
fn build_tls_acceptor_enables_session_tickets_when_requested() {
    let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-tickets");
    let tls = ServerTls {
        cert_path: cert_path.clone(),
        key_path: key_path.clone(),
        additional_certificates: Vec::new(),
        versions: None,
        cipher_suites: None,
        key_exchange_groups: None,
        alpn_protocols: None,
        ocsp_staple_path: None,
        ocsp: rginx_core::OcspConfig::default(),
        session_resumption: Some(true),
        session_tickets: Some(true),
        session_cache_size: Some(2),
        session_ticket_count: Some(4),
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
    assert!(acceptor.config().session_storage.can_cache());
    assert!(acceptor.config().ticketer.enabled());
    assert_eq!(acceptor.config().send_tls13_tickets, 4);

    let storage = &acceptor.config().session_storage;
    assert!(storage.put(vec![0x01], vec![0x0a]));
    assert!(storage.put(vec![0x02], vec![0x0b]));
    assert!(storage.put(vec![0x03], vec![0x0c]));
    let count = storage.get(&[0x01]).iter().count()
        + storage.get(&[0x02]).iter().count()
        + storage.get(&[0x03]).iter().count();
    assert!(count < 3);

    remove_test_cert_pair(cert_path, key_path, temp_dir);
}
