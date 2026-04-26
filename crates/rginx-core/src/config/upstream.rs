use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use super::tls::{ClientIdentity, TlsVersion};

mod selection;
mod types;

pub use types::{
    ActiveHealthCheck, Upstream, UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer,
    UpstreamProtocol, UpstreamSettings, UpstreamTls,
};
