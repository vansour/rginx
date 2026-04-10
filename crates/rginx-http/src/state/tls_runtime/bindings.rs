use super::*;

pub(super) fn tls_binding_snapshots(
    config: &ConfigSnapshot,
    certificates: &[TlsCertificateStatusSnapshot],
) -> (
    Vec<TlsVhostBindingSnapshot>,
    Vec<TlsSniBindingSnapshot>,
    Vec<TlsSniBindingSnapshot>,
    Vec<TlsDefaultCertificateBindingSnapshot>,
) {
    let fingerprint_by_scope = certificates
        .iter()
        .map(|certificate| {
            (
                certificate.scope.clone(),
                certificate.fingerprint_sha256.clone().unwrap_or_else(|| "-".to_string()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut vhost_bindings = Vec::new();
    let mut sni_bindings =
        std::collections::BTreeMap::<(String, String), TlsSniBindingSnapshot>::new();
    let mut default_certificate_bindings = Vec::new();

    for listener in &config.listeners {
        if !listener.tls_enabled() {
            continue;
        }

        for vhost in std::iter::once(&config.default_vhost).chain(config.vhosts.iter()) {
            let Some(certificate_scope) = tls_certificate_scope_for_listener_vhost(listener, vhost)
            else {
                continue;
            };
            let fingerprint = fingerprint_by_scope
                .get(&certificate_scope)
                .cloned()
                .unwrap_or_else(|| "-".to_string());
            let default_selected = listener.server.default_certificate.is_none()
                || listener.server.default_certificate.as_ref().is_some_and(|default_name| {
                    vhost.server_names.iter().any(|name| name == default_name)
                });
            vhost_bindings.push(TlsVhostBindingSnapshot {
                listener_name: listener.name.clone(),
                vhost_id: vhost.id.clone(),
                server_names: vhost.server_names.clone(),
                certificate_scopes: vec![certificate_scope.clone()],
                fingerprints: vec![fingerprint.clone()],
                default_selected,
            });

            for server_name in &vhost.server_names {
                let binding = sni_bindings
                    .entry((listener.name.clone(), server_name.clone()))
                    .or_insert_with(|| TlsSniBindingSnapshot {
                        listener_name: listener.name.clone(),
                        server_name: server_name.clone(),
                        certificate_scopes: Vec::new(),
                        fingerprints: Vec::new(),
                        default_selected,
                    });
                if !binding.certificate_scopes.iter().any(|scope| scope == &certificate_scope) {
                    binding.certificate_scopes.push(certificate_scope.clone());
                }
                if !binding.fingerprints.iter().any(|value| value == &fingerprint) {
                    binding.fingerprints.push(fingerprint.clone());
                }
                binding.default_selected = binding.default_selected || default_selected;
            }
        }

        let Some(default_certificate) = listener.server.default_certificate.as_ref() else {
            continue;
        };
        let Some(vhost) =
            std::iter::once(&config.default_vhost).chain(config.vhosts.iter()).find(|vhost| {
                vhost.server_names.iter().any(|server_name| server_name == default_certificate)
            })
        else {
            continue;
        };
        let Some(certificate_scope) = tls_certificate_scope_for_listener_vhost(listener, vhost)
        else {
            continue;
        };
        let fingerprint = fingerprint_by_scope
            .get(&certificate_scope)
            .cloned()
            .unwrap_or_else(|| "-".to_string());
        default_certificate_bindings.push(TlsDefaultCertificateBindingSnapshot {
            listener_name: listener.name.clone(),
            server_name: default_certificate.clone(),
            certificate_scopes: vec![certificate_scope],
            fingerprints: vec![fingerprint],
        });
    }

    vhost_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.vhost_id.cmp(&right.vhost_id))
    });
    let mut sni_bindings = sni_bindings.into_values().collect::<Vec<_>>();
    sni_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.server_name.cmp(&right.server_name))
    });
    let sni_conflicts = sni_bindings
        .iter()
        .filter(|binding| binding.fingerprints.len() > 1)
        .cloned()
        .collect::<Vec<_>>();
    default_certificate_bindings.sort_by(|left, right| {
        left.listener_name
            .cmp(&right.listener_name)
            .then_with(|| left.server_name.cmp(&right.server_name))
    });

    (vhost_bindings, sni_bindings, sni_conflicts, default_certificate_bindings)
}

fn tls_certificate_scope_for_listener_vhost(
    listener: &Listener,
    vhost: &rginx_core::VirtualHost,
) -> Option<String> {
    if vhost.tls.is_some() {
        Some(format!("vhost:{}", vhost.id))
    } else if listener.server.tls.is_some() {
        Some(format!("listener:{}", listener.name))
    } else {
        None
    }
}
