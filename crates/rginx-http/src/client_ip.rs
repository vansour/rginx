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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsClientIdentity {
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub serial_number: Option<String>,
    pub san_dns_names: Vec<String>,
    pub chain_length: usize,
    pub chain_subjects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPeerAddrs {
    pub socket_peer_addr: SocketAddr,
    pub proxy_protocol_source_addr: Option<SocketAddr>,
    pub tls_client_identity: Option<TlsClientIdentity>,
    pub tls_version: Option<String>,
    pub tls_alpn: Option<String>,
    pub early_data: bool,
}

pub fn resolve_client_address(
    headers: &HeaderMap,
    server: &Server,
    connection: &ConnectionPeerAddrs,
) -> ClientAddress {
    if !server.is_trusted_proxy(connection.socket_peer_addr.ip()) {
        return direct_peer(connection.socket_peer_addr);
    }

    let immediate_peer =
        connection.proxy_protocol_source_addr.unwrap_or(connection.socket_peer_addr);
    let Some(chain) = parse_x_forwarded_for(headers) else {
        return proxied_peer(connection.socket_peer_addr, immediate_peer.ip());
    };

    let forwarded_for = format!(
        "{}, {}",
        chain.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
        immediate_peer.ip()
    );

    ClientAddress {
        peer_addr: connection.socket_peer_addr,
        client_ip: select_client_ip_with_immediate_peer(&chain, immediate_peer.ip(), server),
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

fn proxied_peer(peer_addr: SocketAddr, client_ip: IpAddr) -> ClientAddress {
    ClientAddress {
        peer_addr,
        client_ip,
        forwarded_for: client_ip.to_string(),
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

    if chain.is_empty() { None } else { Some(chain) }
}

fn parse_forwarded_ip(token: &str) -> Option<IpAddr> {
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip);
    }

    token.parse::<SocketAddr>().ok().map(|addr| addr.ip())
}

fn select_client_ip_with_immediate_peer(
    chain: &[IpAddr],
    immediate_peer: IpAddr,
    server: &Server,
) -> IpAddr {
    chain
        .iter()
        .rev()
        .copied()
        .chain(std::iter::once(immediate_peer))
        .find(|ip| !server.is_trusted_proxy(*ip))
        .unwrap_or(chain[0])
}

#[cfg(test)]
mod tests;
