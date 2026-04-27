use http::{HeaderMap, HeaderValue};

use super::super::{ProxyHeaderRenderContext, ProxyHeaderTemplate, ProxyHeaderValue};

#[test]
fn proxy_header_template_renders_request_context_values() {
    let mut headers = HeaderMap::new();
    headers.insert("cf-connecting-ip", HeaderValue::from_static("198.51.100.9"));
    let template = ProxyHeaderTemplate::parse(
        "https://{host}/{scheme}/{client_ip}/{header:cf-connecting-ip}".to_string(),
    )
    .expect("template should parse");
    let context = ProxyHeaderRenderContext {
        original_headers: &headers,
        original_host: Some(&HeaderValue::from_static("dashboard.example.com")),
        upstream_authority: "127.0.0.1:8008",
        client_ip: "203.0.113.10".parse().unwrap(),
        peer_addr: "10.0.0.2:443".parse().unwrap(),
        forwarded_for: "203.0.113.10, 10.0.0.2",
        scheme: "https",
    };

    let value = ProxyHeaderValue::Template(template)
        .render(&context)
        .expect("template should render")
        .expect("template should produce a value");

    assert_eq!(
        value,
        HeaderValue::from_static("https://dashboard.example.com/https/203.0.113.10/198.51.100.9")
    );
}

#[test]
fn proxy_header_template_supports_escaped_braces() {
    let headers = HeaderMap::new();
    let template = ProxyHeaderTemplate::parse("{{\"host\":\"{host}\"}}".to_string()).unwrap();
    let context = ProxyHeaderRenderContext {
        original_headers: &headers,
        original_host: Some(&HeaderValue::from_static("dashboard.example.com")),
        upstream_authority: "127.0.0.1:8008",
        client_ip: "203.0.113.10".parse().unwrap(),
        peer_addr: "10.0.0.2:443".parse().unwrap(),
        forwarded_for: "203.0.113.10, 10.0.0.2",
        scheme: "https",
    };

    let value = ProxyHeaderValue::Template(template)
        .render(&context)
        .expect("template should render")
        .expect("template should produce a value");

    assert_eq!(value, HeaderValue::from_static("{\"host\":\"dashboard.example.com\"}"));
}

#[test]
fn proxy_header_template_rejects_unescaped_closing_braces() {
    let error = ProxyHeaderTemplate::parse("literal }".to_string())
        .expect_err("unescaped closing brace should fail");

    assert!(error.to_string().contains("unescaped closing brace"));
}

#[test]
fn proxy_header_template_preserves_request_header_bytes() {
    let mut headers = HeaderMap::new();
    headers.insert("x-raw", HeaderValue::from_bytes(b"caf\xe9").unwrap());
    let template = ProxyHeaderTemplate::parse("{header:x-raw}".to_string()).unwrap();
    let context = ProxyHeaderRenderContext {
        original_headers: &headers,
        original_host: None,
        upstream_authority: "127.0.0.1:8008",
        client_ip: "203.0.113.10".parse().unwrap(),
        peer_addr: "10.0.0.2:443".parse().unwrap(),
        forwarded_for: "203.0.113.10, 10.0.0.2",
        scheme: "https",
    };

    let value = ProxyHeaderValue::Template(template)
        .render(&context)
        .expect("template should render")
        .expect("template should produce a value");

    assert_eq!(value.as_bytes(), b"caf\xe9");
}
