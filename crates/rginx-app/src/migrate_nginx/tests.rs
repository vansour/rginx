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
