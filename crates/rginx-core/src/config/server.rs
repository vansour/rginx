use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use http::HeaderValue;
use ipnet::IpNet;

use super::{AccessLogFormat, ServerTls};

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
}

#[derive(Debug, Clone)]
pub struct Server {
    pub listen_addr: SocketAddr,
    pub server_header: HeaderValue,
    pub default_certificate: Option<String>,
    pub trusted_proxies: Vec<IpNet>,
    pub keep_alive: bool,
    pub max_headers: Option<usize>,
    pub max_request_body_bytes: Option<usize>,
    pub max_connections: Option<usize>,
    pub header_read_timeout: Option<Duration>,
    pub request_body_read_timeout: Option<Duration>,
    pub response_write_timeout: Option<Duration>,
    pub access_log_format: Option<AccessLogFormat>,
    pub tls: Option<ServerTls>,
}

impl Server {
    pub fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies.iter().any(|cidr| cidr.contains(&ip))
    }
}

pub const DEFAULT_SERVER_HEADER: &str = "rginx";

pub fn default_server_header() -> HeaderValue {
    HeaderValue::from_static(DEFAULT_SERVER_HEADER)
}
