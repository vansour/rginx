use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn migrate_nginx_generates_checkable_rginx_config() {
    let temp_dir = temp_dir("rginx-migrate-test");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let nginx_path = temp_dir.join("nginx.conf");
    let ron_path = temp_dir.join("rginx.ron");

    fs::write(
        &nginx_path,
        r#"
        worker_processes auto;
        events {}
        http {
            upstream backend {
                server 127.0.0.1:19090 weight=2;
                server 127.0.0.1:19091 backup;
            }

            server {
                listen 18080;
                server_name api.example.com;
                client_max_body_size 4m;

                location /api {
                    proxy_pass http://backend;
                    proxy_set_header Host $host;
                    proxy_set_header X-Static-Route api;
                    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
                }
            }
        }
        "#,
    )
    .expect("nginx config should be written");

    let migrate_output = run_rginx([
        "migrate-nginx",
        "--input",
        nginx_path.to_str().unwrap(),
        "--output",
        ron_path.to_str().unwrap(),
    ]);
    assert!(
        migrate_output.status.success(),
        "migration should succeed: {}",
        render_output(&migrate_output)
    );

    let generated = fs::read_to_string(&ron_path).expect("migrated config should exist");
    assert!(generated.contains("listen: \"0.0.0.0:18080\""));
    assert!(generated.contains("server_names: [\"api.example.com\"]"));
    assert!(generated.contains("max_request_body_bytes: Some(4194304)"));
    assert!(generated.contains("preserve_host: Some(true)"));
    assert!(generated.contains("\"X-Static-Route\": \"api\""));
    assert!(!generated.contains("X-Forwarded-For"));

    let check_output = run_rginx(["check", "--config", ron_path.to_str().unwrap()]);
    assert!(
        check_output.status.success(),
        "migrated config should validate: {}",
        render_output(&check_output)
    );

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn migrate_nginx_rejects_proxy_pass_with_uri_path() {
    let temp_dir = temp_dir("rginx-migrate-test");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let nginx_path = temp_dir.join("nginx.conf");

    fs::write(
        &nginx_path,
        r#"
        http {
            server {
                listen 8080;
                location /api {
                    proxy_pass http://backend/internal;
                }
            }
        }
        "#,
    )
    .expect("nginx config should be written");

    let migrate_output = run_rginx(["migrate-nginx", "--input", nginx_path.to_str().unwrap()]);
    assert!(!migrate_output.status.success(), "migration should fail");
    let stderr = String::from_utf8_lossy(&migrate_output.stderr);
    assert!(stderr.contains("contains a URI path"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn migrate_nginx_recognizes_common_ssl_directives() {
    let temp_dir = temp_dir("rginx-migrate-ssl-test");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let nginx_path = temp_dir.join("nginx.conf");
    let ron_path = temp_dir.join("rginx.ron");

    fs::write(
        &nginx_path,
        r#"
        http {
            upstream backend {
                server 127.0.0.1:19443;
            }

            server {
                listen 443 ssl;
                server_name app.example.com;
                ssl_certificate /etc/nginx/certs/app.crt;
                ssl_certificate_key /etc/nginx/certs/app.key;
                ssl_protocols TLSv1.2 TLSv1.3;
                ssl_stapling_file /var/cache/nginx/app.ocsp;
                ssl_session_tickets off;
                ssl_client_certificate /etc/nginx/certs/client-ca.crt;
                ssl_verify_client on;
                ssl_verify_depth 3;
                ssl_crl /etc/nginx/certs/client-auth.crl.pem;

                location / {
                    proxy_pass https://backend;
                    proxy_ssl_verify off;
                    proxy_ssl_verify_depth 2;
                    proxy_ssl_crl /etc/nginx/certs/upstream.crl.pem;
                    proxy_ssl_name backend.internal;
                    proxy_ssl_server_name off;
                    proxy_ssl_protocols TLSv1.3;
                }
            }
        }
        "#,
    )
    .expect("nginx config should be written");

    let migrate_output = run_rginx([
        "migrate-nginx",
        "--input",
        nginx_path.to_str().unwrap(),
        "--output",
        ron_path.to_str().unwrap(),
    ]);
    assert!(
        migrate_output.status.success(),
        "migration should succeed: {}",
        render_output(&migrate_output)
    );

    let generated = fs::read_to_string(&ron_path).expect("migrated config should exist");
    assert!(generated.contains("listen: \"0.0.0.0:443\""));
    assert!(generated.contains("tls: Some(ServerTlsConfig("));
    assert!(generated.contains("cert_path: \"/etc/nginx/certs/app.crt\""));
    assert!(generated.contains("key_path: \"/etc/nginx/certs/app.key\""));
    assert!(generated.contains("versions: Some([Tls12, Tls13])"));
    assert!(generated.contains("ocsp_staple_path: Some(\"/var/cache/nginx/app.ocsp\")"));
    assert!(generated.contains("session_tickets: Some(false)"));
    assert!(generated.contains("client_auth: Some(ServerClientAuthConfig("));
    assert!(generated.contains("ca_cert_path: \"/etc/nginx/certs/client-ca.crt\""));
    assert!(generated.contains("verify_depth: Some(3)"));
    assert!(generated.contains("crl_path: Some(\"/etc/nginx/certs/client-auth.crl.pem\")"));
    assert!(generated.contains("tls: Some(UpstreamTlsConfig("));
    assert!(generated.contains("verify: Insecure"));
    assert!(generated.contains("versions: Some([Tls13])"));
    assert!(generated.contains("verify_depth: Some(2)"));
    assert!(generated.contains("crl_path: Some(\"/etc/nginx/certs/upstream.crl.pem\")"));
    assert!(generated.contains("server_name: Some(false)"));
    assert!(generated.contains("server_name_override: Some(\"backend.internal\")"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn migrate_nginx_warns_when_ssl_stapling_is_enabled_without_a_cache_file_mapping() {
    let temp_dir = temp_dir("rginx-migrate-ssl-warning-test");
    fs::create_dir_all(&temp_dir).expect("temp dir should exist");
    let nginx_path = temp_dir.join("nginx.conf");

    fs::write(
        &nginx_path,
        r#"
        http {
            server {
                listen 443 ssl;
                server_name app.example.com;
                ssl_certificate /etc/nginx/certs/app.crt;
                ssl_certificate_key /etc/nginx/certs/app.key;
                ssl_stapling on;

                location / {
                    proxy_pass http://127.0.0.1:18080;
                }
            }
        }
        "#,
    )
    .expect("nginx config should be written");

    let migrate_output = run_rginx(["migrate-nginx", "--input", nginx_path.to_str().unwrap()]);
    assert!(
        migrate_output.status.success(),
        "migration should succeed: {}",
        render_output(&migrate_output)
    );
    let stdout = String::from_utf8_lossy(&migrate_output.stdout);
    assert!(stdout.contains("ssl_stapling on"));
    assert!(stdout.contains("ssl_stapling_file"));

    let _ = fs::remove_dir_all(temp_dir);
}

fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> Output {
    let mut command = Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }

    command.output().expect("rginx should run")
}

fn binary_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/rginx")
        .canonicalize()
        .expect("rginx binary path should resolve")
}

fn render_output(output: &Output) -> String {
    format!(
        "status={:?}\nstdout=\n{}\nstderr=\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).expect("time should move forward").as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{nanos}-{id}"))
}
