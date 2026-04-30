use super::*;

#[test]
fn check_reports_managed_acme_configuration() {
    let temp_dir = temp_dir("rginx-check-acme");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("acme.ron");
    let cert_path = temp_dir.join("managed.crt");
    let key_path = temp_dir.join("managed.key");

    let mut params =
        CertificateParams::new(vec!["api.example.com".to_string()]).expect("certificate params");
    params.distinguished_name.push(DnType::CommonName, "api.example.com");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key_pair = KeyPair::generate().expect("key pair should generate");
    let certificate = params.self_signed(&key_pair).expect("certificate should self-sign");
    fs::write(&cert_path, certificate.pem()).expect("certificate should be written");
    fs::write(&key_path, key_pair.serialize_pem()).expect("private key should be written");

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    acme: Some(AcmeConfig(\n        directory_url: \"https://acme-staging-v02.api.letsencrypt.org/directory\",\n        contacts: [\"mailto:ops@example.com\"],\n        state_dir: \"state/acme\",\n        renew_before_days: Some(21),\n        poll_interval_secs: Some(600),\n    )),\n    listeners: [\n        ListenerConfig(\n            name: \"http\",\n            listen: \"127.0.0.1:80\",\n        ),\n        ListenerConfig(\n            name: \"https\",\n            listen: \"127.0.0.1:443\",\n            tls: Some(ServerTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n            )),\n        ),\n    ],\n    server: ServerConfig(),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api\\n\"),\n                    ),\n                ),\n            ],\n            tls: Some(VirtualHostTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n                acme: Some(VirtualHostAcmeConfig(\n                    domains: [\"api.example.com\"],\n                )),\n            )),\n        ),\n    ],\n)\n",
            cert_path.display().to_string(),
            key_path.display().to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
        ),
    )
    .expect("managed ACME config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("acme_details=enabled=true"));
    assert!(
        stdout.contains("directory_url=https://acme-staging-v02.api.letsencrypt.org/directory")
    );
    assert!(stdout.contains(&format!("state_dir={}", temp_dir.join("state/acme").display())));
    assert!(stdout.contains("renew_before_days=21"));
    assert!(stdout.contains("poll_interval_secs=600"));
    assert!(stdout.contains("managed_certificates=1"));
    assert!(stdout.contains("acme_certificate scope=servers[0]"));
    assert!(stdout.contains("domains=api.example.com"));
    assert!(stdout.contains("challenge_type=http-01"));
}
