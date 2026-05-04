use super::super::*;

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
fn proxy_uri_normalizes_dot_segments_and_duplicate_slashes() {
    let peer = resolved_peer_from_url("http://127.0.0.1:9000");

    let uri = build_proxy_uri(&peer, &"/api//v1/../items/?x=1".parse().unwrap(), None).unwrap();
    assert_eq!(uri, "http://127.0.0.1:9000/api/items/?x=1".parse::<http::Uri>().unwrap());
}

#[test]
fn proxy_uri_strips_prefix_after_normalization() {
    let peer = resolved_peer_from_url("http://127.0.0.1:9000");

    let uri =
        build_proxy_uri(&peer, &"/api/./v1/../items/detail?id=7".parse().unwrap(), Some("/api"))
            .unwrap();
    assert_eq!(uri, "http://127.0.0.1:9000/items/detail?id=7".parse::<http::Uri>().unwrap());
}

#[test]
fn proxy_uri_preserves_asterisk_form() {
    let peer = resolved_peer_from_url("http://127.0.0.1:9000");

    let uri = build_proxy_uri(&peer, &"*".parse().unwrap(), None).unwrap();
    assert_eq!(uri.scheme_str(), Some("http"));
    assert_eq!(uri.authority().map(|authority| authority.as_str()), Some("127.0.0.1:9000"));
    assert_eq!(uri.path(), "*");
}
