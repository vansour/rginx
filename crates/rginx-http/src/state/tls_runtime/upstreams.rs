use super::*;

pub(super) fn upstream_tls_status_snapshots(
    config: &ConfigSnapshot,
) -> Vec<UpstreamTlsStatusSnapshot> {
    let mut upstreams = config.upstreams.values().collect::<Vec<_>>();
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));
    upstreams.into_iter().map(|upstream| upstream_tls_status_snapshot(upstream.as_ref())).collect()
}

pub(super) fn upstream_tls_status_snapshot(
    upstream: &rginx_core::Upstream,
) -> UpstreamTlsStatusSnapshot {
    UpstreamTlsStatusSnapshot {
        upstream_name: upstream.name.clone(),
        protocol: upstream.protocol.as_str().to_string(),
        verify_mode: crate::proxy::upstream_tls_verify_label(&upstream.tls).to_string(),
        tls_versions: upstream.tls_versions.as_ref().map(|versions| {
            versions
                .iter()
                .map(|version| match version {
                    rginx_core::TlsVersion::Tls12 => "TLS1.2".to_string(),
                    rginx_core::TlsVersion::Tls13 => "TLS1.3".to_string(),
                })
                .collect()
        }),
        server_name_enabled: upstream.server_name,
        server_name_override: upstream.server_name_override.clone(),
        verify_depth: upstream.server_verify_depth,
        crl_configured: upstream.server_crl_path.is_some(),
        client_identity_configured: upstream.client_identity.is_some(),
    }
}
