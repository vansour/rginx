use std::collections::BTreeSet;

pub(super) struct TlsCheckDetails {
    pub(super) listener_tls_profiles: usize,
    pub(super) vhost_tls_overrides: usize,
    pub(super) sni_name_count: usize,
    pub(super) certificate_bundle_count: usize,
    pub(super) default_certificates: Vec<String>,
    pub(super) expiring_certificates: Vec<String>,
    pub(super) reloadable_fields: Vec<String>,
    pub(super) restart_required_fields: Vec<String>,
    pub(super) listeners: Vec<rginx_http::TlsListenerStatusSnapshot>,
    pub(super) certificates: Vec<rginx_http::TlsCertificateStatusSnapshot>,
    pub(super) ocsp: Vec<rginx_http::TlsOcspStatusSnapshot>,
    pub(super) vhost_bindings: Vec<rginx_http::TlsVhostBindingSnapshot>,
    pub(super) sni_bindings: Vec<TlsSniBindingCheck>,
    pub(super) sni_conflicts: Vec<TlsSniBindingCheck>,
    pub(super) default_certificate_bindings: Vec<TlsDefaultCertificateBindingCheck>,
}

pub(super) struct TlsSniBindingCheck {
    pub(super) listener_name: String,
    pub(super) server_name: String,
    pub(super) fingerprints: Vec<String>,
    pub(super) scopes: Vec<String>,
    pub(super) default_selected: bool,
}

pub(super) struct TlsDefaultCertificateBindingCheck {
    pub(super) listener_name: String,
    pub(super) server_name: String,
    pub(super) fingerprints: Vec<String>,
    pub(super) scopes: Vec<String>,
}

pub(super) fn tls_check_details(config: &rginx_config::ConfigSnapshot) -> TlsCheckDetails {
    let tls = rginx_http::tls_runtime_snapshot_for_config(config);
    let listener_tls_profiles =
        config.listeners.iter().filter(|listener| listener.server.tls.is_some()).count();
    let vhost_tls_overrides = std::iter::once(&config.default_vhost)
        .chain(config.vhosts.iter())
        .filter(|vhost| vhost.tls.is_some())
        .count();
    let sni_name_count = if listener_tls_profiles == 0 && vhost_tls_overrides == 0 {
        0
    } else {
        let mut names = BTreeSet::new();
        names.extend(config.default_vhost.server_names.iter().cloned());
        for vhost in config.vhosts.iter().filter(|vhost| vhost.tls.is_some()) {
            names.extend(vhost.server_names.iter().cloned());
        }
        names.len()
    };
    let certificate_bundle_count = config
        .listeners
        .iter()
        .filter_map(|listener| listener.server.tls.as_ref())
        .map(|tls| 1 + tls.additional_certificates.len())
        .sum::<usize>()
        + std::iter::once(&config.default_vhost)
            .chain(config.vhosts.iter())
            .filter_map(|vhost| vhost.tls.as_ref())
            .map(|tls| 1 + tls.additional_certificates.len())
            .sum::<usize>();
    let default_certificates = config
        .listeners
        .iter()
        .filter_map(|listener| {
            listener.server.tls.as_ref().and_then(|_| {
                listener
                    .server
                    .default_certificate
                    .as_ref()
                    .map(|name| format!("{}={}", listener.name, name))
            })
        })
        .collect();
    let expiring_certificates = tls
        .certificates
        .iter()
        .filter_map(|certificate| {
            certificate.expires_in_days.and_then(|days| {
                ((0..=30).contains(&days)).then(|| format!("{}:{}d", certificate.scope, days))
            })
        })
        .collect();
    let (sni_bindings, sni_conflicts, default_certificate_bindings) = (
        tls.sni_bindings
            .iter()
            .map(|binding| TlsSniBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
                default_selected: binding.default_selected,
            })
            .collect(),
        tls.sni_conflicts
            .iter()
            .map(|binding| TlsSniBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
                default_selected: binding.default_selected,
            })
            .collect(),
        tls.default_certificate_bindings
            .iter()
            .map(|binding| TlsDefaultCertificateBindingCheck {
                listener_name: binding.listener_name.clone(),
                server_name: binding.server_name.clone(),
                fingerprints: binding.fingerprints.clone(),
                scopes: binding.certificate_scopes.clone(),
            })
            .collect(),
    );

    TlsCheckDetails {
        listener_tls_profiles,
        vhost_tls_overrides,
        sni_name_count,
        certificate_bundle_count,
        default_certificates,
        expiring_certificates,
        reloadable_fields: tls.reload_boundary.reloadable_fields,
        restart_required_fields: tls.reload_boundary.restart_required_fields,
        listeners: tls.listeners,
        vhost_bindings: tls.vhost_bindings,
        ocsp: tls.ocsp,
        certificates: tls.certificates,
        sni_bindings,
        sni_conflicts,
        default_certificate_bindings,
    }
}
