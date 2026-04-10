use super::convert::parse_proxy_pass_target;
use super::migrate_source;
use super::parser::parse_size;

#[test]
fn parse_size_supports_nginx_suffixes() {
    assert_eq!(parse_size("8k").unwrap(), 8 * 1024);
    assert_eq!(parse_size("10m").unwrap(), 10 * 1024 * 1024);
    assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(parse_size("512").unwrap(), 512);
}

#[test]
fn parse_proxy_pass_rejects_uri_paths() {
    let error = parse_proxy_pass_target("http://backend/api").expect_err("path should fail");
    assert!(error.to_string().contains("contains a URI path"));
}

#[test]
fn migrate_source_renders_supported_subset() {
    let migrated = migrate_source(
        r#"
        worker_processes auto;
        events {}
        http {
            upstream backend {
                server 10.0.0.10:8080 weight=3;
                server 10.0.0.11:8080 backup;
            }

            server {
                listen 8080;
                server_name api.example.com;
                client_max_body_size 10m;

                location = /healthz {
                    proxy_pass http://backend;
                    proxy_set_header Host $host;
                    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
                    proxy_set_header X-Trace-Static static-value;
                }
            }
        }
        "#,
        "inline.conf",
    )
    .expect("migration should succeed");

    assert!(migrated.ron.contains("listen: \"0.0.0.0:8080\""));
    assert!(migrated.ron.contains("server_names: [\"api.example.com\"]"));
    assert!(migrated.ron.contains("max_request_body_bytes: Some(10485760)"));
    assert!(migrated.ron.contains("upstream: \"backend\""));
    assert!(migrated.ron.contains("preserve_host: Some(true)"));
    assert!(migrated.ron.contains("\"X-Trace-Static\": \"static-value\""));
    assert!(!migrated.ron.contains("X-Forwarded-For"));
}

#[test]
fn migrate_source_preserves_listen_address_when_quic_flag_follows_it() {
    let migrated = migrate_source(
        r#"
        http {
            upstream backend {
                server 10.0.0.10:8443;
            }

            server {
                listen 443 quic reuseport;

                location / {
                    proxy_pass https://backend;
                }
            }
        }
        "#,
        "inline.conf",
    )
    .expect("migration should succeed");

    assert!(migrated.ron.contains("listen: \"0.0.0.0:443\""));
    assert!(!migrated.ron.contains("listen: \"quic\""));
    assert!(
        migrated.warnings.iter().any(|warning| warning.contains("listen ... quic")),
        "warnings should mention the ignored quic token: {:?}",
        migrated.warnings
    );
}

#[test]
fn migrate_source_renders_additional_downstream_certificate_pairs() {
    let migrated = migrate_source(
        r#"
        http {
            upstream backend {
                server 10.0.0.10:8080;
            }

            server {
                listen 443 ssl;
                ssl_certificate /etc/nginx/certs/rsa.crt;
                ssl_certificate_key /etc/nginx/certs/rsa.key;
                ssl_certificate /etc/nginx/certs/ecdsa.crt;
                ssl_certificate_key /etc/nginx/certs/ecdsa.key;

                location / {
                    proxy_pass http://backend;
                }
            }
        }
        "#,
        "inline.conf",
    )
    .expect("migration should succeed");

    assert!(migrated.ron.contains("cert_path: \"/etc/nginx/certs/rsa.crt\""));
    assert!(migrated.ron.contains("key_path: \"/etc/nginx/certs/rsa.key\""));
    assert!(migrated.ron.contains("additional_certificates: Some(["));
    assert!(migrated.ron.contains("cert_path: \"/etc/nginx/certs/ecdsa.crt\""));
    assert!(migrated.ron.contains("key_path: \"/etc/nginx/certs/ecdsa.key\""));
}

#[test]
fn migrate_source_does_not_use_insecure_tls_shorthand_when_other_tls_fields_are_present() {
    let migrated = migrate_source(
        r#"
        http {
            upstream backend {
                server 10.0.0.10:8443;
            }

            server {
                listen 8080;

                location / {
                    proxy_pass https://backend;
                    proxy_ssl_verify off;
                    proxy_ssl_verify_depth 2;
                    proxy_ssl_crl /etc/nginx/upstream.crl;
                }
            }
        }
        "#,
        "inline.conf",
    )
    .expect("migration should succeed");

    assert!(!migrated.ron.contains("tls: Some(Insecure),"));
    assert!(migrated.ron.contains("verify: Insecure,"));
    assert!(migrated.ron.contains("verify_depth: Some(2),"));
    assert!(migrated.ron.contains("crl_path: Some(\"/etc/nginx/upstream.crl\"),"));
}
