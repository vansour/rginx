use super::*;
use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response};
use tempfile::tempdir;

use crate::cache::{CacheLookup, CacheRequest};
use crate::handler::full_body;

#[tokio::test]
async fn status_snapshot_reports_runtime_summary() {
    let shared = SharedState::from_config_path(
        PathBuf::from("/etc/rginx/rginx.ron"),
        snapshot("127.0.0.1:8080"),
    )
    .expect("shared state should build");

    let status = shared.status_snapshot().await;
    assert_eq!(status.revision, 0);
    assert_eq!(status.config_path, Some(PathBuf::from("/etc/rginx/rginx.ron")));
    assert_eq!(status.listeners.len(), 1);
    assert_eq!(status.listeners[0].listener_id, "default");
    assert_eq!(status.listeners[0].listener_name, "default");
    assert_eq!(status.listeners[0].listen_addr, "127.0.0.1:8080".parse().unwrap());
    assert_eq!(status.listeners[0].binding_count, 1);
    assert!(!status.listeners[0].http3_enabled);
    assert!(!status.listeners[0].tls_enabled);
    assert!(!status.listeners[0].proxy_protocol_enabled);
    assert!(!status.listeners[0].access_log_format_configured);
    assert_eq!(status.listeners[0].bindings.len(), 1);
    assert_eq!(status.listeners[0].bindings[0].transport, "tcp");
    assert_eq!(status.listeners[0].bindings[0].protocols, vec!["http1".to_string()]);
    assert_eq!(status.total_vhosts, 1);
    assert_eq!(status.total_routes, 0);
    assert_eq!(status.total_upstreams, 0);
    assert!(!status.tls_enabled);
    assert_eq!(status.http3_active_connections, 0);
    assert_eq!(status.http3_active_request_streams, 0);
    assert_eq!(status.http3_retry_issued_total, 0);
    assert_eq!(status.http3_retry_failed_total, 0);
    assert_eq!(status.http3_request_accept_errors_total, 0);
    assert_eq!(status.http3_request_resolve_errors_total, 0);
    assert_eq!(status.http3_request_body_stream_errors_total, 0);
    assert_eq!(status.http3_response_stream_errors_total, 0);
    assert_eq!(status.tls.listeners.len(), 1);
    assert_eq!(status.listeners[0].http3_runtime, None);
    assert!(!status.tls.listeners[0].http3_enabled);
    assert_eq!(status.tls.listeners[0].http3_listen_addr, None);
    assert!(status.tls.listeners[0].http3_versions.is_empty());
    assert!(status.tls.listeners[0].http3_alpn_protocols.is_empty());
    assert_eq!(status.tls.listeners[0].session_resumption_enabled, None);
    assert_eq!(status.tls.listeners[0].session_tickets_enabled, None);
    assert_eq!(status.tls.listeners[0].session_cache_size, None);
    assert_eq!(status.tls.listeners[0].session_ticket_count, None);
    assert_eq!(status.tls.certificates.len(), 0);
    assert_eq!(status.tls.expiring_certificate_count, 0);
    assert_eq!(status.mtls.configured_listeners, 0);
    assert_eq!(status.mtls.authenticated_requests, 0);
    assert_eq!(status.active_connections, 0);
    assert_eq!(status.reload.attempts_total, 0);
}

#[tokio::test]
async fn status_snapshot_reports_http3_listener_bindings() {
    let mut config = snapshot("127.0.0.1:8443");
    config.listeners[0].tls_termination_enabled = true;
    config.listeners[0].http3 = Some(rginx_core::ListenerHttp3 {
        listen_addr: "127.0.0.1:8443".parse().unwrap(),
        advertise_alt_svc: true,
        alt_svc_max_age: Duration::from_secs(7200),
        max_concurrent_streams: 128,
        stream_buffer_size: 64 * 1024,
        active_connection_id_limit: 2,
        retry: false,
        host_key_path: None,
        gso: false,
        early_data_enabled: false,
    });
    let shared = SharedState::from_config(config).expect("shared state should build");

    let status = shared.status_snapshot().await;
    assert_eq!(status.listeners.len(), 1);
    assert_eq!(status.listeners[0].binding_count, 2);
    assert!(status.listeners[0].http3_enabled);
    assert_eq!(status.listeners[0].bindings.len(), 2);
    assert_eq!(status.listeners[0].bindings[0].transport, "tcp");
    assert_eq!(
        status.listeners[0].bindings[0].protocols,
        vec!["http1".to_string(), "http2".to_string()]
    );
    assert_eq!(status.listeners[0].bindings[1].transport, "udp");
    assert_eq!(status.listeners[0].bindings[1].protocols, vec!["http3".to_string()]);
    assert_eq!(status.listeners[0].bindings[1].advertise_alt_svc, Some(true));
    assert_eq!(status.listeners[0].bindings[1].alt_svc_max_age_secs, Some(7200));
    assert_eq!(status.listeners[0].bindings[1].http3_max_concurrent_streams, Some(128));
    assert_eq!(status.listeners[0].bindings[1].http3_stream_buffer_size, Some(64 * 1024));
    assert_eq!(status.listeners[0].bindings[1].http3_active_connection_id_limit, Some(2));
    assert_eq!(status.listeners[0].bindings[1].http3_retry, Some(false));
    assert_eq!(status.listeners[0].bindings[1].http3_host_key_path, None);
    assert_eq!(status.listeners[0].bindings[1].http3_gso, Some(false));
    assert_eq!(status.http3_active_connections, 0);
    assert_eq!(status.http3_active_request_streams, 0);
    assert_eq!(status.http3_retry_issued_total, 0);
    assert_eq!(status.http3_retry_failed_total, 0);
    assert_eq!(status.http3_request_accept_errors_total, 0);
    assert_eq!(status.http3_request_resolve_errors_total, 0);
    assert_eq!(status.http3_request_body_stream_errors_total, 0);
    assert_eq!(status.http3_response_stream_errors_total, 0);
    let http3_runtime = status.listeners[0]
        .http3_runtime
        .as_ref()
        .expect("http3 runtime snapshot should be present");
    assert_eq!(http3_runtime.active_connections, 0);
    assert_eq!(http3_runtime.active_request_streams, 0);
    assert_eq!(http3_runtime.retry_issued_total, 0);
    assert_eq!(http3_runtime.retry_failed_total, 0);
    assert_eq!(http3_runtime.request_accept_errors_total, 0);
    assert_eq!(http3_runtime.request_resolve_errors_total, 0);
    assert_eq!(http3_runtime.request_body_stream_errors_total, 0);
    assert_eq!(http3_runtime.response_stream_errors_total, 0);
    assert_eq!(http3_runtime.connection_close_version_mismatch_total, 0);
    assert_eq!(http3_runtime.connection_close_transport_error_total, 0);
    assert_eq!(http3_runtime.connection_close_connection_closed_total, 0);
    assert_eq!(http3_runtime.connection_close_application_closed_total, 0);
    assert_eq!(http3_runtime.connection_close_reset_total, 0);
    assert_eq!(http3_runtime.connection_close_timed_out_total, 0);
    assert_eq!(http3_runtime.connection_close_locally_closed_total, 0);
    assert_eq!(http3_runtime.connection_close_cids_exhausted_total, 0);
    assert_eq!(status.tls.listeners.len(), 1);
    assert!(status.tls.listeners[0].http3_enabled);
    assert_eq!(status.tls.listeners[0].http3_listen_addr, Some("127.0.0.1:8443".parse().unwrap()));
    assert_eq!(status.tls.listeners[0].http3_versions, vec!["TLS1.3".to_string()]);
    assert_eq!(status.tls.listeners[0].http3_alpn_protocols, vec!["h3".to_string()]);
    assert_eq!(status.tls.listeners[0].http3_max_concurrent_streams, Some(128));
    assert_eq!(status.tls.listeners[0].http3_stream_buffer_size, Some(64 * 1024));
    assert_eq!(status.tls.listeners[0].http3_active_connection_id_limit, Some(2));
    assert_eq!(status.tls.listeners[0].http3_retry, Some(false));
    assert_eq!(status.tls.listeners[0].http3_host_key_path, None);
    assert_eq!(status.tls.listeners[0].http3_gso, Some(false));
}

#[tokio::test]
async fn http3_runtime_telemetry_tracks_status_and_traffic_snapshots() {
    let mut config = snapshot("127.0.0.1:8443");
    config.listeners[0].tls_termination_enabled = true;
    config.listeners[0].http3 = Some(rginx_core::ListenerHttp3 {
        listen_addr: "127.0.0.1:8443".parse().unwrap(),
        advertise_alt_svc: true,
        alt_svc_max_age: Duration::from_secs(7200),
        max_concurrent_streams: 128,
        stream_buffer_size: 64 * 1024,
        active_connection_id_limit: 2,
        retry: true,
        host_key_path: None,
        gso: false,
        early_data_enabled: false,
    });
    let shared = SharedState::from_config(config).expect("shared state should build");

    {
        let _connection =
            shared.retain_http3_connection("default").expect("http3 connection guard should exist");
        let _stream = shared
            .retain_http3_request_stream("default")
            .expect("http3 request stream guard should exist");
        shared.record_http3_retry_issued("default");
        shared.record_http3_retry_failed("default");
        shared.record_http3_request_accept_error("default");
        shared.record_http3_request_resolve_error("default");
        shared.record_http3_request_body_stream_error("default");
        shared.record_http3_response_stream_error("default");
        shared.record_http3_connection_close("default", quinn::ConnectionError::VersionMismatch);
        shared.record_http3_connection_close("default", quinn::ConnectionError::Reset);
        shared.record_http3_connection_close("default", quinn::ConnectionError::TimedOut);
        shared.record_http3_connection_close("default", quinn::ConnectionError::LocallyClosed);
        shared.record_http3_connection_close("default", quinn::ConnectionError::CidsExhausted);

        let status = shared.status_snapshot().await;
        assert_eq!(status.http3_active_connections, 1);
        assert_eq!(status.http3_active_request_streams, 1);
        assert_eq!(status.http3_retry_issued_total, 1);
        assert_eq!(status.http3_retry_failed_total, 1);
        assert_eq!(status.http3_request_accept_errors_total, 1);
        assert_eq!(status.http3_request_resolve_errors_total, 1);
        assert_eq!(status.http3_request_body_stream_errors_total, 1);
        assert_eq!(status.http3_response_stream_errors_total, 1);
        let http3_runtime =
            status.listeners[0].http3_runtime.as_ref().expect("http3 runtime should be present");
        assert_eq!(http3_runtime.active_connections, 1);
        assert_eq!(http3_runtime.active_request_streams, 1);
        assert_eq!(http3_runtime.retry_issued_total, 1);
        assert_eq!(http3_runtime.retry_failed_total, 1);
        assert_eq!(http3_runtime.request_accept_errors_total, 1);
        assert_eq!(http3_runtime.request_resolve_errors_total, 1);
        assert_eq!(http3_runtime.request_body_stream_errors_total, 1);
        assert_eq!(http3_runtime.response_stream_errors_total, 1);
        assert_eq!(http3_runtime.connection_close_version_mismatch_total, 1);
        assert_eq!(http3_runtime.connection_close_reset_total, 1);
        assert_eq!(http3_runtime.connection_close_timed_out_total, 1);
        assert_eq!(http3_runtime.connection_close_locally_closed_total, 1);
        assert_eq!(http3_runtime.connection_close_cids_exhausted_total, 1);
    }

    let status = shared.status_snapshot().await;
    assert_eq!(status.http3_active_connections, 0);
    assert_eq!(status.http3_active_request_streams, 0);

    let traffic = shared.traffic_stats_snapshot();
    let http3_runtime = traffic.listeners[0]
        .http3_runtime
        .as_ref()
        .expect("traffic listener should include http3 runtime");
    assert_eq!(http3_runtime.active_connections, 0);
    assert_eq!(http3_runtime.active_request_streams, 0);
    assert_eq!(http3_runtime.retry_issued_total, 1);
    assert_eq!(http3_runtime.retry_failed_total, 1);
    assert_eq!(http3_runtime.request_accept_errors_total, 1);
    assert_eq!(http3_runtime.request_resolve_errors_total, 1);
    assert_eq!(http3_runtime.request_body_stream_errors_total, 1);
    assert_eq!(http3_runtime.response_stream_errors_total, 1);
    assert_eq!(http3_runtime.connection_close_version_mismatch_total, 1);
    assert_eq!(http3_runtime.connection_close_reset_total, 1);
    assert_eq!(http3_runtime.connection_close_timed_out_total, 1);
    assert_eq!(http3_runtime.connection_close_locally_closed_total, 1);
    assert_eq!(http3_runtime.connection_close_cids_exhausted_total, 1);
}

#[tokio::test]
async fn mtls_status_snapshot_excludes_non_mtls_listener_handshake_failures() {
    let shared =
        SharedState::from_config(snapshot("127.0.0.1:8080")).expect("shared state should build");

    shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::MissingClientCert);

    let status = shared.status_snapshot().await;
    let counters = shared.counters_snapshot();

    assert_eq!(counters.downstream_tls_handshake_failures, 1);
    assert_eq!(status.mtls.configured_listeners, 0);
    assert_eq!(status.mtls.handshake_failures_total, 0);
}

#[tokio::test]
async fn current_listener_returns_retired_listener_and_exposes_runtime_metadata() {
    let config_path = PathBuf::from("/etc/rginx/rginx.ron");
    let shared = SharedState::from_config_path(config_path.clone(), snapshot("127.0.0.1:8080"))
        .expect("shared state should build");

    let mut retired = snapshot("127.0.0.1:9090")
        .listeners
        .into_iter()
        .next()
        .expect("snapshot should contain a listener");
    retired.id = "retired".to_string();
    retired.name = "retired".to_string();
    shared.retire_listener_runtime(&retired);

    let expected_path = config_path;
    assert_eq!(shared.config_path().map(|path| path.as_path()), Some(expected_path.as_path()));
    assert_eq!(shared.current_revision().await, 0);

    let listener =
        shared.current_listener("retired").await.expect("retired listener should be returned");
    assert_eq!(listener.id, "retired");
    assert_eq!(listener.name, "retired");
    assert_eq!(listener.server.listen_addr, "127.0.0.1:9090".parse().unwrap());
}

#[tokio::test]
async fn status_snapshot_reports_cache_zone_stats() {
    let temp = tempdir().expect("cache temp dir should exist");
    let shared = SharedState::from_config(snapshot_with_cache_zone(
        "127.0.0.1:8080",
        temp.path().to_path_buf(),
    ))
    .expect("shared state should build");

    let active = shared.snapshot().await;
    let policy = default_route_cache_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/status-cache")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match active.cache.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            CacheLookup::Hit(_) => panic!("empty cache should miss"),
            CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
            CacheLookup::Bypass(status) => {
                panic!("cacheable request should not bypass: {status:?}")
            }
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("cached"))
        .expect("response should build");
    let _ = active.cache.store_response(context, response).await;

    let status = shared.status_snapshot().await;
    assert_eq!(status.cache.zones.len(), 1);
    assert_eq!(status.cache.zones[0].zone_name, "default");
    assert_eq!(status.cache.zones[0].entry_count, 1);
    assert_eq!(status.cache.zones[0].current_size_bytes, 6);
}
