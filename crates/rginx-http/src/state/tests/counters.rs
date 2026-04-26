use super::*;

#[test]
fn counters_snapshot_tracks_connections_requests_and_response_buckets() {
    let shared =
        SharedState::from_config(snapshot("127.0.0.1:8080")).expect("shared state should build");

    shared.record_connection_accepted("default");
    shared.record_connection_rejected("default");
    shared.record_downstream_request("default", "server", None);
    shared.record_downstream_request("default", "server", None);
    shared.record_downstream_response("default", "server", None, StatusCode::OK);
    shared.record_downstream_response("default", "server", None, StatusCode::NOT_FOUND);
    shared.record_downstream_response("default", "server", None, StatusCode::BAD_GATEWAY);

    let counters = shared.counters_snapshot();
    assert_eq!(counters.downstream_connections_accepted, 1);
    assert_eq!(counters.downstream_connections_rejected, 1);
    assert_eq!(counters.downstream_requests, 2);
    assert_eq!(counters.downstream_responses, 3);
    assert_eq!(counters.downstream_responses_2xx, 1);
    assert_eq!(counters.downstream_responses_4xx, 1);
    assert_eq!(counters.downstream_responses_5xx, 1);
    assert_eq!(counters.downstream_mtls_authenticated_requests, 0);
    assert_eq!(counters.downstream_tls_handshake_failures, 0);
}

#[test]
fn counters_snapshot_tracks_mtls_activity() {
    let shared =
        SharedState::from_config(snapshot("127.0.0.1:8080")).expect("shared state should build");

    shared.record_mtls_handshake_success("default", true);
    shared.record_mtls_request("default", true);
    shared.record_mtls_request("default", false);
    shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::MissingClientCert);
    shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::UnknownCa);
    shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::BadCertificate);
    shared.record_tls_handshake_failure("default", TlsHandshakeFailureReason::Other);

    let counters = shared.counters_snapshot();
    assert_eq!(counters.downstream_mtls_authenticated_connections, 1);
    assert_eq!(counters.downstream_mtls_authenticated_requests, 1);
    assert_eq!(counters.downstream_mtls_anonymous_requests, 1);
    assert_eq!(counters.downstream_tls_handshake_failures, 4);
    assert_eq!(counters.downstream_tls_handshake_failures_missing_client_cert, 1);
    assert_eq!(counters.downstream_tls_handshake_failures_unknown_ca, 1);
    assert_eq!(counters.downstream_tls_handshake_failures_bad_certificate, 1);
    assert_eq!(counters.downstream_tls_handshake_failures_other, 1);
}
