use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsReloadBoundarySnapshot {
    pub reloadable_fields: Vec<String>,
    pub restart_required_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsListenerStatusSnapshot {
    pub listener_id: String,
    pub listener_name: String,
    pub listen_addr: std::net::SocketAddr,
    pub tls_enabled: bool,
    pub http3_enabled: bool,
    pub http3_listen_addr: Option<std::net::SocketAddr>,
    pub default_certificate: Option<String>,
    pub versions: Option<Vec<String>>,
    pub alpn_protocols: Vec<String>,
    pub http3_versions: Vec<String>,
    pub http3_alpn_protocols: Vec<String>,
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
    pub session_resumption_enabled: Option<bool>,
    pub session_tickets_enabled: Option<bool>,
    pub session_cache_size: Option<usize>,
    pub session_ticket_count: Option<usize>,
    pub client_auth_mode: Option<String>,
    pub client_auth_verify_depth: Option<u32>,
    pub client_auth_crl_configured: bool,
    pub sni_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsCertificateStatusSnapshot {
    pub scope: String,
    pub cert_path: PathBuf,
    pub server_names: Vec<String>,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub serial_number: Option<String>,
    pub san_dns_names: Vec<String>,
    pub fingerprint_sha256: Option<String>,
    pub subject_key_identifier: Option<String>,
    pub authority_key_identifier: Option<String>,
    pub is_ca: Option<bool>,
    pub path_len_constraint: Option<u32>,
    pub key_usage: Option<String>,
    pub extended_key_usage: Vec<String>,
    pub not_before_unix_ms: Option<u64>,
    pub not_after_unix_ms: Option<u64>,
    pub expires_in_days: Option<i64>,
    pub chain_length: usize,
    pub chain_subjects: Vec<String>,
    pub chain_diagnostics: Vec<String>,
    pub selected_as_default_for_listeners: Vec<String>,
    pub ocsp_staple_configured: bool,
    pub additional_certificate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsOcspStatusSnapshot {
    pub scope: String,
    pub cert_path: PathBuf,
    pub ocsp_staple_path: Option<PathBuf>,
    pub responder_urls: Vec<String>,
    pub nonce_mode: String,
    pub responder_policy: String,
    pub cache_loaded: bool,
    pub cache_size_bytes: Option<usize>,
    pub cache_modified_unix_ms: Option<u64>,
    pub auto_refresh_enabled: bool,
    pub last_refresh_unix_ms: Option<u64>,
    pub refreshes_total: u64,
    pub failures_total: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsOcspRefreshSpec {
    pub scope: String,
    pub cert_path: PathBuf,
    pub ocsp_staple_path: Option<PathBuf>,
    pub responder_urls: Vec<String>,
    pub auto_refresh_enabled: bool,
    pub ocsp_nonce_mode: rginx_core::OcspNonceMode,
    pub ocsp_responder_policy: rginx_core::OcspResponderPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsVhostBindingSnapshot {
    pub listener_name: String,
    pub vhost_id: String,
    pub server_names: Vec<String>,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsSniBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
    pub default_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsDefaultCertificateBindingSnapshot {
    pub listener_name: String,
    pub server_name: String,
    pub certificate_scopes: Vec<String>,
    pub fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsRuntimeSnapshot {
    pub listeners: Vec<TlsListenerStatusSnapshot>,
    pub certificates: Vec<TlsCertificateStatusSnapshot>,
    pub ocsp: Vec<TlsOcspStatusSnapshot>,
    pub vhost_bindings: Vec<TlsVhostBindingSnapshot>,
    pub sni_bindings: Vec<TlsSniBindingSnapshot>,
    pub sni_conflicts: Vec<TlsSniBindingSnapshot>,
    pub default_certificate_bindings: Vec<TlsDefaultCertificateBindingSnapshot>,
    pub reload_boundary: TlsReloadBoundarySnapshot,
    pub expiring_certificate_count: usize,
}
