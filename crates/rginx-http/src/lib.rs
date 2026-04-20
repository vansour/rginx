mod client_ip;
mod compression;
pub mod handler;
mod pki;
pub mod proxy;
pub mod rate_limit;
pub mod router;
pub mod server;
pub mod state;
mod timeout;
mod tls;
mod transition;

pub const MAX_OCSP_RESPONSE_BYTES: usize = 128 * 1024;

pub fn install_default_crypto_provider() {
    tls::install_default_crypto_provider();
}

pub use client_ip::TlsClientIdentity;
pub use proxy::{PeerHealthSnapshot, UpstreamHealthSnapshot};
pub use server::serve;
pub use state::{
    GrpcTrafficSnapshot, HttpCountersSnapshot, ListenerStatsSnapshot, MtlsStatusSnapshot,
    ReloadOutcomeSnapshot, ReloadResultSnapshot, ReloadStatusSnapshot, RouteStatsSnapshot,
    RuntimeListenerBindingSnapshot, RuntimeListenerSnapshot, RuntimeStatusSnapshot, SharedState,
    SnapshotDeltaSnapshot, SnapshotModule, TlsCertificateStatusSnapshot,
    TlsDefaultCertificateBindingSnapshot, TlsListenerStatusSnapshot, TlsOcspRefreshSpec,
    TlsOcspStatusSnapshot, TlsReloadBoundarySnapshot, TlsRuntimeSnapshot, TlsSniBindingSnapshot,
    TlsVhostBindingSnapshot, TrafficStatsSnapshot, UpstreamPeerStatsSnapshot,
    UpstreamStatsSnapshot, UpstreamTlsStatusSnapshot, VhostStatsSnapshot,
    tls_ocsp_refresh_specs_for_config, tls_reloadable_fields, tls_restart_required_fields,
    tls_runtime_snapshot_for_config,
};
pub use tls::{
    build_ocsp_request_for_certificate, build_ocsp_request_for_certificate_with_options,
    validate_ocsp_response_for_certificate, validate_ocsp_response_for_certificate_with_options,
};
pub use transition::{
    ConfigTransitionBoundary, ConfigTransitionKind, ConfigTransitionPlan,
    config_transition_boundary, plan_config_transition, validate_config_transition,
};

#[cfg(test)]
#[ctor::ctor]
fn install_test_crypto_provider() {
    install_default_crypto_provider();
}
