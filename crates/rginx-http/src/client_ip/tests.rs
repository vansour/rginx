use http::{HeaderMap, HeaderValue};
use rginx_core::Server;

use super::{ClientIpSource, ConnectionPeerAddrs, resolve_client_address};

#[test]
fn untrusted_peer_ignores_spoofed_x_forwarded_for() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));

    let client = resolve_client_address(
        &headers,
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "192.0.2.10:4000".parse().unwrap(),
            proxy_protocol_source_addr: None,
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.client_ip.to_string(), "192.0.2.10");
    assert_eq!(client.forwarded_for, "192.0.2.10");
    assert_eq!(client.source, ClientIpSource::SocketPeer);
}

#[test]
fn trusted_peer_uses_last_untrusted_x_forwarded_for_entry() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9, 10.1.2.3"));

    let client = resolve_client_address(
        &headers,
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "10.2.3.4:4000".parse().unwrap(),
            proxy_protocol_source_addr: None,
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.client_ip.to_string(), "198.51.100.9");
    assert_eq!(client.forwarded_for, "198.51.100.9, 10.1.2.3, 10.2.3.4");
    assert_eq!(client.source, ClientIpSource::XForwardedFor);
}

#[test]
fn trusted_peer_keeps_leftmost_entry_when_chain_is_all_trusted() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("10.1.2.3, 10.2.3.4"));

    let client = resolve_client_address(
        &headers,
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "10.3.4.5:4000".parse().unwrap(),
            proxy_protocol_source_addr: None,
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.client_ip.to_string(), "10.1.2.3");
    assert_eq!(client.source, ClientIpSource::XForwardedFor);
}

#[test]
fn malformed_x_forwarded_for_falls_back_to_peer() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));

    let client = resolve_client_address(
        &headers,
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "10.2.3.4:4000".parse().unwrap(),
            proxy_protocol_source_addr: None,
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.client_ip.to_string(), "10.2.3.4");
    assert_eq!(client.source, ClientIpSource::SocketPeer);
}

#[test]
fn x_forwarded_for_entries_may_include_socket_addresses() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9:1234"));

    let client = resolve_client_address(
        &headers,
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "10.2.3.4:4000".parse().unwrap(),
            proxy_protocol_source_addr: None,
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.client_ip.to_string(), "198.51.100.9");
    assert_eq!(client.source, ClientIpSource::XForwardedFor);
}

#[test]
fn trusted_proxy_protocol_source_is_used_when_xff_is_absent() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };

    let client = resolve_client_address(
        &HeaderMap::new(),
        &server,
        &ConnectionPeerAddrs {
            socket_peer_addr: "10.2.3.4:4000".parse().unwrap(),
            proxy_protocol_source_addr: Some("198.51.100.9:1234".parse().unwrap()),
            tls_client_identity: None,
            tls_version: None,
            tls_alpn: None,
            early_data: false,
        },
    );

    assert_eq!(client.peer_addr.to_string(), "10.2.3.4:4000");
    assert_eq!(client.client_ip.to_string(), "198.51.100.9");
    assert_eq!(client.forwarded_for, "198.51.100.9");
}
