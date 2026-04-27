use super::*;

#[test]
fn proxy_uri_keeps_path_and_query() {
    let peer = resolved_peer_from_url("http://127.0.0.1:9000");

    let uri = build_proxy_uri(&peer, &"/api/demo?x=1".parse().unwrap(), None).unwrap();
    assert_eq!(uri, "http://127.0.0.1:9000/api/demo?x=1".parse::<http::Uri>().unwrap());
}

#[test]
fn proxy_uri_keeps_https_scheme() {
    let peer = resolved_peer_from_url("https://example.com");

    let uri = build_proxy_uri(&peer, &"/healthz".parse().unwrap(), None).unwrap();
    assert_eq!(uri, "https://example.com/healthz".parse::<http::Uri>().unwrap());
}

#[test]
fn proxy_uri_uses_upstream_authority_not_resolved_dial_authority() {
    let mut peer = resolved_peer_from_url("https://httpbingo.org");
    peer.dial_authority = "203.0.113.10:443".to_string();
    peer.socket_addr = "203.0.113.10:443".parse().unwrap();

    let uri = build_proxy_uri(&peer, &"/anything?demo=1".parse().unwrap(), None).unwrap();

    assert_eq!(uri, "https://httpbingo.org/anything?demo=1".parse::<http::Uri>().unwrap());
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
fn sanitize_request_headers_renders_dynamic_proxy_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("dashboard.example.com"));
    headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.20"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "203.0.113.20".parse().unwrap(),
        forwarded_for: "203.0.113.20, 10.2.3.4".to_string(),
        source: ClientIpSource::ClientIpHeader,
    };
    let proxy_set_headers = vec![
        ("nz-realip".parse().unwrap(), ProxyHeaderValue::ClientIp),
        (
            "origin".parse().unwrap(),
            ProxyHeaderValue::Template(
                ProxyHeaderTemplate::parse("https://{host}".to_string()).unwrap(),
            ),
        ),
        (
            "x-cf-ip".parse().unwrap(),
            ProxyHeaderValue::RequestHeader("cf-connecting-ip".parse().unwrap()),
        ),
    ];

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:8008",
        Some(HeaderValue::from_static("dashboard.example.com")),
        &client_address,
        "https",
        true,
        &proxy_set_headers,
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "dashboard.example.com");
    assert_eq!(headers.get("nz-realip").unwrap(), "203.0.113.20");
    assert_eq!(headers.get("origin").unwrap(), "https://dashboard.example.com");
    assert_eq!(headers.get("x-cf-ip").unwrap(), "203.0.113.20");
}

#[test]
fn sanitize_request_headers_keeps_target_when_dynamic_source_is_missing() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert("authorization", HeaderValue::from_static("Bearer client"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };
    let proxy_set_headers = vec![(
        "authorization".parse().unwrap(),
        ProxyHeaderValue::RequestHeader("x-forward-auth".parse().unwrap()),
    )];

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "https",
        false,
        &proxy_set_headers,
        None,
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get("authorization").unwrap(), "Bearer client");
}

#[test]
fn sanitize_request_headers_removes_explicit_remove_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert("authorization", HeaderValue::from_static("Bearer client"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };
    let proxy_set_headers = vec![("authorization".parse().unwrap(), ProxyHeaderValue::Remove)];

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "https",
        false,
        &proxy_set_headers,
        None,
    )
    .expect("header sanitization should succeed");

    assert!(!headers.contains_key("authorization"));
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

#[test]
fn removes_redundant_host_for_auto_https_authority_pseudo_header() {
    let peer = resolved_peer_from_url("https://mirrors.ocf.berkeley.edu");
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("mirrors.ocf.berkeley.edu"));

    remove_redundant_host_header_for_authority_pseudo_header(
        &mut headers,
        &peer,
        UpstreamProtocol::Auto,
    );

    assert!(headers.get(HOST).is_none());
}

#[test]
fn keeps_host_for_http1_only_upstream_requests() {
    let peer = resolved_peer_from_url("https://mirrors.ocf.berkeley.edu");
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("mirrors.ocf.berkeley.edu"));

    remove_redundant_host_header_for_authority_pseudo_header(
        &mut headers,
        &peer,
        UpstreamProtocol::Http1,
    );

    assert_eq!(headers.get(HOST).unwrap(), "mirrors.ocf.berkeley.edu");
}

#[test]
fn removes_redundant_host_for_http2_upstream_requests() {
    let peer = resolved_peer_from_url("http://grpc.internal:50051");
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("grpc.internal:50051"));

    remove_redundant_host_header_for_authority_pseudo_header(
        &mut headers,
        &peer,
        UpstreamProtocol::Http2,
    );

    assert!(headers.get(HOST).is_none());
}

#[test]
fn keeps_host_for_auto_cleartext_upstream_requests() {
    let peer = resolved_peer_from_url("http://mirrors.ocf.berkeley.edu");
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("mirrors.ocf.berkeley.edu"));

    remove_redundant_host_header_for_authority_pseudo_header(
        &mut headers,
        &peer,
        UpstreamProtocol::Auto,
    );

    assert_eq!(headers.get(HOST).unwrap(), "mirrors.ocf.berkeley.edu");
}

#[test]
fn keeps_non_authority_host_overrides_for_proxy_compatibility() {
    let peer = resolved_peer_from_url("https://mirrors.ocf.berkeley.edu");
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("download.example"));

    remove_redundant_host_header_for_authority_pseudo_header(
        &mut headers,
        &peer,
        UpstreamProtocol::Auto,
    );

    assert_eq!(headers.get(HOST).unwrap(), "download.example");
}
