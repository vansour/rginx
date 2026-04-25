mod accept;
mod connection;
mod graceful;
mod http3;
mod proxy_protocol;

#[cfg(test)]
mod tests;

pub use accept::serve;
pub use http3::{bind_http3_endpoint, bind_http3_endpoint_with_socket, serve_http3};

#[doc(hidden)]
pub fn parse_proxy_protocol_v1_for_fuzzing(
    header: &str,
    remote_addr: std::net::SocketAddr,
    trust_remote_addr: bool,
) -> std::io::Result<Option<std::net::SocketAddr>> {
    proxy_protocol::parse_proxy_protocol_v1(header, remote_addr, trust_remote_addr)
}
