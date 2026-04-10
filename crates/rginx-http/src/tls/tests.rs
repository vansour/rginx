use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rginx_core::{ServerTls, TlsCipherSuite, TlsKeyExchangeGroup, VirtualHost, VirtualHostTls};
use rustls::{CipherSuite, NamedGroup};

use super::{best_matching_wildcard_certificates, build_tls_acceptor};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

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

#[test]
fn wildcard_sni_selection_prefers_more_specific_patterns() {
    let certs = vec![Arc::new(dummy_certified_key())];
    let by_name = HashMap::from([
        ("*.example.com".to_string(), certs.clone()),
        ("*.api.example.com".to_string(), certs.clone()),
    ]);

    let selected = best_matching_wildcard_certificates(&by_name, "edge.api.example.com")
        .expect("more specific wildcard should match");
    assert_eq!(selected.len(), 1);
    assert!(best_matching_wildcard_certificates(&by_name, "example.com").is_none());
}

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

fn dummy_certified_key() -> rustls::sign::CertifiedKey {
    let cert_path = std::env::temp_dir().join("rginx-unused-test-cert.pem");
    let key_path = std::env::temp_dir().join("rginx-unused-test-key.pem");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("dummy cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("dummy key should be written");
    let certified = self::super::certificates::load_certified_key_bundle(
        &rginx_core::ServerCertificateBundle {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            ocsp_staple_path: None,
            ocsp: rginx_core::OcspConfig::default(),
        },
    )
    .expect("dummy certified key should load");
    let _ = fs::remove_file(cert_path);
    let _ = fs::remove_file(key_path);
    Arc::unwrap_or_clone(certified)
}

fn write_test_cert_pair(
    prefix: &str,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("{prefix}-{unique}"));
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");
    (cert_path, key_path, temp_dir)
}

fn remove_test_cert_pair(
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
    temp_dir: std::path::PathBuf,
) {
    fs::remove_file(cert_path).expect("test cert should be removed");
    fs::remove_file(key_path).expect("test key should be removed");
    fs::remove_dir(temp_dir).expect("temp dir should be removed");
}
