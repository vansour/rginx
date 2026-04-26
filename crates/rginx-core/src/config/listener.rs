use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use super::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerTransportKind {
    Tcp,
    Udp,
}

impl ListenerTransportKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListenerApplicationProtocol {
    Http1,
    Http2,
    Http3,
}

impl ListenerApplicationProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http1 => "http1",
            Self::Http2 => "http2",
            Self::Http3 => "http3",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenerHttp3 {
    pub listen_addr: SocketAddr,
    pub advertise_alt_svc: bool,
    pub alt_svc_max_age: Duration,
    pub max_concurrent_streams: usize,
    pub stream_buffer_size: usize,
    pub active_connection_id_limit: u32,
    pub retry: bool,
    pub host_key_path: Option<PathBuf>,
    pub gso: bool,
    pub early_data_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ListenerTransportBinding {
    pub name: &'static str,
    pub kind: ListenerTransportKind,
    pub listen_addr: SocketAddr,
    pub protocols: Vec<ListenerApplicationProtocol>,
    pub advertise_alt_svc: bool,
    pub alt_svc_max_age: Option<Duration>,
    pub http3_max_concurrent_streams: Option<usize>,
    pub http3_stream_buffer_size: Option<usize>,
    pub http3_active_connection_id_limit: Option<u32>,
    pub http3_retry: Option<bool>,
    pub http3_host_key_path: Option<PathBuf>,
    pub http3_gso: Option<bool>,
    pub http3_early_data_enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Listener {
    pub id: String,
    pub name: String,
    pub server: Server,
    pub tls_termination_enabled: bool,
    pub proxy_protocol_enabled: bool,
    pub http3: Option<ListenerHttp3>,
}

impl Listener {
    pub fn tls_enabled(&self) -> bool {
        self.tls_termination_enabled
    }

    pub fn http3_enabled(&self) -> bool {
        self.http3.is_some()
    }

    pub fn binding_count(&self) -> usize {
        1 + usize::from(self.http3.is_some())
    }

    pub fn transport_bindings(&self) -> Vec<ListenerTransportBinding> {
        let mut bindings = vec![ListenerTransportBinding {
            name: "tcp",
            kind: ListenerTransportKind::Tcp,
            listen_addr: self.server.listen_addr,
            protocols: if self.tls_enabled() {
                vec![ListenerApplicationProtocol::Http1, ListenerApplicationProtocol::Http2]
            } else {
                vec![ListenerApplicationProtocol::Http1]
            },
            advertise_alt_svc: false,
            alt_svc_max_age: None,
            http3_max_concurrent_streams: None,
            http3_stream_buffer_size: None,
            http3_active_connection_id_limit: None,
            http3_retry: None,
            http3_host_key_path: None,
            http3_gso: None,
            http3_early_data_enabled: None,
        }];

        if let Some(http3) = &self.http3 {
            bindings.push(ListenerTransportBinding {
                name: "udp",
                kind: ListenerTransportKind::Udp,
                listen_addr: http3.listen_addr,
                protocols: vec![ListenerApplicationProtocol::Http3],
                advertise_alt_svc: http3.advertise_alt_svc,
                alt_svc_max_age: Some(http3.alt_svc_max_age),
                http3_max_concurrent_streams: Some(http3.max_concurrent_streams),
                http3_stream_buffer_size: Some(http3.stream_buffer_size),
                http3_active_connection_id_limit: Some(http3.active_connection_id_limit),
                http3_retry: Some(http3.retry),
                http3_host_key_path: http3.host_key_path.clone(),
                http3_gso: Some(http3.gso),
                http3_early_data_enabled: Some(http3.early_data_enabled),
            });
        }

        bindings
    }
}
