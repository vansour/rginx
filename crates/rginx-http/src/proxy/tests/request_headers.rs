use super::*;

#[test]
fn proxy_uri_keeps_path_and_query() {
    let peer = UpstreamPeer {
        url: "http://127.0.0.1:9000".to_string(),
        scheme: "http".to_string(),
        authority: "127.0.0.1:9000".to_string(),
        weight: 1,
        backup: false,
    };

    let uri = build_proxy_uri(&peer, &"/api/demo?x=1".parse().unwrap(), None).unwrap();
    assert_eq!(uri, "http://127.0.0.1:9000/api/demo?x=1".parse::<http::Uri>().unwrap());
}

#[test]
fn proxy_uri_keeps_https_scheme() {
    let peer = UpstreamPeer {
        url: "https://example.com".to_string(),
        scheme: "https".to_string(),
        authority: "example.com".to_string(),
        weight: 1,
        backup: false,
    };

    let uri = build_proxy_uri(&peer, &"/healthz".parse().unwrap(), None).unwrap();
    assert_eq!(uri, "https://example.com/healthz".parse::<http::Uri>().unwrap());
}

#[test]
fn sanitize_request_headers_overwrites_x_forwarded_for_with_sanitized_chain() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert("x-forwarded-for", HeaderValue::from_static("spoofed"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9, 10.1.2.3, 10.2.3.4".to_string(),
        source: ClientIpSource::XForwardedFor,
    };

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "https",
        false,
        &[],
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
    assert_eq!(headers.get("x-forwarded-host").unwrap(), "client.example");
    assert_eq!(headers.get("x-forwarded-for").unwrap(), "198.51.100.9, 10.1.2.3, 10.2.3.4");
    assert_eq!(headers.get("x-forwarded-proto").unwrap(), "https");
}

#[test]
fn sanitize_request_headers_preserves_upgrade_handshake() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert(http::header::CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
    headers.insert(http::header::UPGRADE, HeaderValue::from_static("websocket"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "http",
        false,
        &[],
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
    assert_eq!(headers.get(http::header::CONNECTION).unwrap(), "upgrade");
    assert_eq!(headers.get(http::header::UPGRADE).unwrap(), "websocket");
}

#[test]
fn sanitize_request_headers_preserves_te_trailers() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert(http::header::TE, HeaderValue::from_static("trailers"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "https",
        false,
        &[],
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
    assert_eq!(headers.get(http::header::TE).unwrap(), "trailers");
}

#[test]
fn sanitize_request_headers_drops_non_trailers_te_tokens() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert(http::header::TE, HeaderValue::from_static("trailers, gzip"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "https",
        false,
        &[],
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
    assert!(headers.get(http::header::TE).is_none());
}
