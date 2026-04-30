use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{
    AcmeRuntimeSnapshot, CacheStatsSnapshot, MtlsStatusSnapshot, ReloadStatusSnapshot,
    TlsRuntimeSnapshot, UpstreamTlsStatusSnapshot,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatusSnapshot {
    pub revision: u64,
    pub config_path: Option<PathBuf>,
    pub listeners: Vec<RuntimeListenerSnapshot>,
    pub worker_threads: Option<usize>,
    pub accept_workers: usize,
    pub total_vhosts: usize,
    pub total_routes: usize,
    pub total_upstreams: usize,
    pub tls_enabled: bool,
    pub http3_active_connections: usize,
    pub http3_active_request_streams: usize,
    pub http3_retry_issued_total: u64,
    pub http3_retry_failed_total: u64,
    pub http3_request_accept_errors_total: u64,
    pub http3_request_resolve_errors_total: u64,
    pub http3_request_body_stream_errors_total: u64,
    pub http3_response_stream_errors_total: u64,
    pub http3_early_data_enabled_listeners: usize,
    pub http3_early_data_accepted_requests: u64,
    pub http3_early_data_rejected_requests: u64,
    pub acme: AcmeRuntimeSnapshot,
    pub tls: TlsRuntimeSnapshot,
    pub mtls: MtlsStatusSnapshot,
    pub upstream_tls: Vec<UpstreamTlsStatusSnapshot>,
    pub cache: CacheStatsSnapshot,
    pub active_connections: usize,
    pub reload: ReloadStatusSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeListenerSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: std::net::SocketAddr,
    pub binding_count: usize,
    pub http3_enabled: bool,
    pub tls_enabled: bool,
    pub proxy_protocol_enabled: bool,
    pub default_certificate: Option<String>,
    pub keep_alive: bool,
    pub max_connections: Option<usize>,
    pub access_log_format_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_runtime: Option<Http3ListenerRuntimeSnapshot>,
    pub bindings: Vec<RuntimeListenerBindingSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeListenerBindingSnapshot {
    pub binding_name: String,
    pub transport: String,
    pub listen_addr: std::net::SocketAddr,
    pub protocols: Vec<String>,
    pub worker_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_port_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertise_alt_svc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_svc_max_age_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_max_concurrent_streams: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_stream_buffer_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_active_connection_id_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_retry: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_host_key_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_gso: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http3_early_data_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Http3ListenerRuntimeSnapshot {
    pub active_connections: usize,
    pub active_request_streams: usize,
    pub retry_issued_total: u64,
    pub retry_failed_total: u64,
    pub request_accept_errors_total: u64,
    pub request_resolve_errors_total: u64,
    pub request_body_stream_errors_total: u64,
    pub response_stream_errors_total: u64,
    pub connection_close_version_mismatch_total: u64,
    pub connection_close_transport_error_total: u64,
    pub connection_close_connection_closed_total: u64,
    pub connection_close_application_closed_total: u64,
    pub connection_close_reset_total: u64,
    pub connection_close_timed_out_total: u64,
    pub connection_close_locally_closed_total: u64,
    pub connection_close_cids_exhausted_total: u64,
}
