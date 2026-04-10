use super::*;

mod bindings;
mod certificates;
mod listeners;
mod ocsp;
mod reload_boundary;
mod upstreams;

use bindings::tls_binding_snapshots;
use certificates::tls_certificate_status_snapshots;
use listeners::tls_listener_status_snapshots;
use ocsp::tls_ocsp_status_snapshots;

#[cfg(test)]
pub(crate) use certificates::inspect_certificate;
pub use ocsp::tls_ocsp_refresh_specs_for_config;
pub use reload_boundary::{tls_reloadable_fields, tls_restart_required_fields};

pub fn tls_runtime_snapshot_for_config(config: &ConfigSnapshot) -> TlsRuntimeSnapshot {
    tls_runtime_snapshot_for_config_with_ocsp_statuses(config, None)
}

pub(super) fn tls_runtime_snapshot_for_config_with_ocsp_statuses(
    config: &ConfigSnapshot,
    ocsp_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> TlsRuntimeSnapshot {
    let listeners = tls_listener_status_snapshots(config);
    let certificates = tls_certificate_status_snapshots(config);
    let expiring_certificate_count = certificates
        .iter()
        .filter(|certificate| {
            certificate.expires_in_days.is_some_and(|days| days <= TLS_EXPIRY_WARNING_DAYS)
        })
        .count();
    let ocsp = tls_ocsp_status_snapshots(config, ocsp_statuses);
    let (vhost_bindings, sni_bindings, sni_conflicts, default_certificate_bindings) =
        tls_binding_snapshots(config, &certificates);

    TlsRuntimeSnapshot {
        listeners,
        certificates,
        ocsp,
        vhost_bindings,
        sni_bindings,
        sni_conflicts,
        default_certificate_bindings,
        reload_boundary: TlsReloadBoundarySnapshot {
            reloadable_fields: tls_reloadable_fields(),
            restart_required_fields: tls_restart_required_fields(),
        },
        expiring_certificate_count,
    }
}

pub(super) fn upstream_tls_status_snapshots(
    config: &ConfigSnapshot,
) -> Vec<UpstreamTlsStatusSnapshot> {
    upstreams::upstream_tls_status_snapshots(config)
}

pub(super) fn upstream_tls_status_snapshot(
    upstream: &rginx_core::Upstream,
) -> UpstreamTlsStatusSnapshot {
    upstreams::upstream_tls_status_snapshot(upstream)
}
