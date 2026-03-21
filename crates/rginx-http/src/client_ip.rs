use std::net::{IpAddr, SocketAddr};

use http::HeaderMap;
use rginx_core::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIpSource {
    SocketPeer,
    XForwardedFor,
}

impl ClientIpSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SocketPeer => "socket_peer",
            Self::XForwardedFor => "x_forwarded_for",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAddress {
    pub peer_addr: SocketAddr,
    pub client_ip: IpAddr,
    pub forwarded_for: String,
    pub source: ClientIpSource,
}

pub fn resolve_client_address(
    headers: &HeaderMap,
    server: &Server,
    peer_addr: SocketAddr,
) -> ClientAddress {
    if !server.is_trusted_proxy(peer_addr.ip()) {
        return direct_peer(peer_addr);
    }

    let Some(chain) = parse_x_forwarded_for(headers) else {
        return direct_peer(peer_addr);
    };

    let forwarded_for = format!(
        "{}, {}",
        chain.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
        peer_addr.ip()
    );

    ClientAddress {
        peer_addr,
        client_ip: select_client_ip(&chain, server),
        forwarded_for,
        source: ClientIpSource::XForwardedFor,
    }
}

fn direct_peer(peer_addr: SocketAddr) -> ClientAddress {
    ClientAddress {
        peer_addr,
        client_ip: peer_addr.ip(),
        forwarded_for: peer_addr.ip().to_string(),
        source: ClientIpSource::SocketPeer,
    }
}

fn parse_x_forwarded_for(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut chain = Vec::new();

    for value in headers.get_all("x-forwarded-for") {
        let value = value.to_str().ok()?;
        for token in value.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }

            chain.push(parse_forwarded_ip(token)?);
        }
    }

    if chain.is_empty() {
        None
    } else {
        Some(chain)
    }
}

fn parse_forwarded_ip(token: &str) -> Option<IpAddr> {
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip);
    }

    token.parse::<SocketAddr>().ok().map(|addr| addr.ip())
}

fn select_client_ip(chain: &[IpAddr], server: &Server) -> IpAddr {
    chain.iter().rev().copied().find(|ip| !server.is_trusted_proxy(*ip)).unwrap_or(chain[0])
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue};
    use rginx_core::Server;

    use super::{resolve_client_address, ClientIpSource};

    #[test]
    fn untrusted_peer_ignores_spoofed_x_forwarded_for() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));

        let client = resolve_client_address(&headers, &server, "192.0.2.10:4000".parse().unwrap());

        assert_eq!(client.client_ip.to_string(), "192.0.2.10");
        assert_eq!(client.forwarded_for, "192.0.2.10");
        assert_eq!(client.source, ClientIpSource::SocketPeer);
    }

    #[test]
    fn trusted_peer_uses_last_untrusted_x_forwarded_for_entry() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9, 10.1.2.3"));

        let client = resolve_client_address(&headers, &server, "10.2.3.4:4000".parse().unwrap());

        assert_eq!(client.client_ip.to_string(), "198.51.100.9");
        assert_eq!(client.forwarded_for, "198.51.100.9, 10.1.2.3, 10.2.3.4");
        assert_eq!(client.source, ClientIpSource::XForwardedFor);
    }

    #[test]
    fn trusted_peer_keeps_leftmost_entry_when_chain_is_all_trusted() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.1.2.3, 10.2.3.4"));

        let client = resolve_client_address(&headers, &server, "10.3.4.5:4000".parse().unwrap());

        assert_eq!(client.client_ip.to_string(), "10.1.2.3");
        assert_eq!(client.source, ClientIpSource::XForwardedFor);
    }

    #[test]
    fn malformed_x_forwarded_for_falls_back_to_peer() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));

        let client = resolve_client_address(&headers, &server, "10.2.3.4:4000".parse().unwrap());

        assert_eq!(client.client_ip.to_string(), "10.2.3.4");
        assert_eq!(client.source, ClientIpSource::SocketPeer);
    }

    #[test]
    fn x_forwarded_for_entries_may_include_socket_addresses() {
        let server = Server {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            trusted_proxies: vec!["10.0.0.0/8".parse().unwrap()],
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            tls: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9:1234"));

        let client = resolve_client_address(&headers, &server, "10.2.3.4:4000".parse().unwrap());

        assert_eq!(client.client_ip.to_string(), "198.51.100.9");
        assert_eq!(client.source, ClientIpSource::XForwardedFor);
    }
}
