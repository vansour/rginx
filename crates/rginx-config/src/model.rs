use serde::Deserialize;

mod cache;
mod listener;
mod route;
mod runtime;
mod server;
mod tls;
mod upstream;
mod vhost;

pub use cache::{CacheRouteConfig, CacheZoneConfig};
pub use listener::{Http3Config, ListenerConfig};
pub use route::{
    HandlerConfig, LocationConfig, MatcherConfig, ProxyHeaderDynamicValueConfig,
    ProxyHeaderValueConfig, RouteBufferingPolicyConfig, RouteCompressionPolicyConfig,
};
pub use runtime::RuntimeConfig;
pub use server::ServerConfig;
pub use tls::{
    OcspConfig, OcspNonceModeConfig, OcspResponderPolicyConfig, ServerCertificateBundleConfig,
    ServerClientAuthConfig, ServerClientAuthModeConfig, ServerTlsConfig, TlsCipherSuiteConfig,
    TlsKeyExchangeGroupConfig, TlsVersionConfig, VirtualHostTlsConfig,
};
pub use upstream::{
    UpstreamConfig, UpstreamDnsConfig, UpstreamLoadBalanceConfig, UpstreamPeerConfig,
    UpstreamProtocolConfig, UpstreamTlsConfig, UpstreamTlsModeConfig,
};
pub use vhost::VirtualHostConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub listeners: Vec<ListenerConfig>,
    #[serde(default)]
    pub cache_zones: Vec<CacheZoneConfig>,
    pub server: ServerConfig,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)]
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub servers: Vec<VirtualHostConfig>,
}
