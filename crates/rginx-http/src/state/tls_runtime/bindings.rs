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

        let mut listener_vhost_bindings = Vec::new();
        let mut listener_sni_bindings =
            std::collections::BTreeMap::<String, TlsSniBindingSnapshot>::new();

        for vhost in std::iter::once(&config.default_vhost).chain(config.vhosts.iter()) {
            let Some(certificate_scope) = tls_certificate_scope_for_listener_vhost(listener, vhost)
            else {
                continue;
            };
            let fingerprint = fingerprint_by_scope
                .get(&certificate_scope)
                .cloned()
                .unwrap_or_else(|| "-".to_string());
            listener_vhost_bindings.push(TlsVhostBindingSnapshot {
                listener_name: listener.name.clone(),
                vhost_id: vhost.id.clone(),
                server_names: vhost.server_names.clone(),
                certificate_scopes: vec![certificate_scope.clone()],
                fingerprints: vec![fingerprint.clone()],
                default_selected: false,
            });

            for server_name in &vhost.server_names {
                let binding =
                    listener_sni_bindings.entry(server_name.clone()).or_insert_with(|| {
                        TlsSniBindingSnapshot {
                            listener_name: listener.name.clone(),
                            server_name: server_name.clone(),
                            certificate_scopes: Vec::new(),
                            fingerprints: Vec::new(),
                            default_selected: false,
                        }
                    });
                if !binding.certificate_scopes.iter().any(|scope| scope == &certificate_scope) {
                    binding.certificate_scopes.push(certificate_scope.clone());
                }
                if !binding.fingerprints.iter().any(|value| value == &fingerprint) {
                    binding.fingerprints.push(fingerprint.clone());
                }
            }
        }

        let (default_scope, default_binding) =
            default_certificate_selection(listener, &listener_sni_bindings);
        if let Some(default_scope) = default_scope {
            for binding in &mut listener_vhost_bindings {
                binding.default_selected =
                    binding.certificate_scopes.iter().any(|scope| scope == &default_scope);
            }
            for binding in listener_sni_bindings.values_mut() {
                binding.default_selected =
                    binding.certificate_scopes.iter().any(|scope| scope == &default_scope);
            }
        }
        if let Some(default_binding) = default_binding {
            default_certificate_bindings.push(default_binding);
        }

        vhost_bindings.extend(listener_vhost_bindings);
        sni_bindings.extend(
            listener_sni_bindings
                .into_iter()
                .map(|(server_name, binding)| ((listener.name.clone(), server_name), binding)),
        );
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

fn default_certificate_selection(
    listener: &Listener,
    listener_sni_bindings: &std::collections::BTreeMap<String, TlsSniBindingSnapshot>,
) -> (Option<String>, Option<TlsDefaultCertificateBindingSnapshot>) {
    if let Some(default_certificate) = listener.server.default_certificate.as_ref()
        && let Some(binding) = listener_sni_bindings.get(default_certificate)
    {
        return (
            binding.certificate_scopes.first().cloned(),
            Some(TlsDefaultCertificateBindingSnapshot {
                listener_name: listener.name.clone(),
                server_name: default_certificate.clone(),
                certificate_scopes: binding.certificate_scopes.clone(),
                fingerprints: binding.fingerprints.clone(),
            }),
        );
    }

    if listener.server.tls.is_some() {
        return (Some(format!("listener:{}", listener.name)), None);
    }

    if listener_sni_bindings.len() == 1
        && let Some((server_name, binding)) = listener_sni_bindings.iter().next()
    {
        return (
            binding.certificate_scopes.first().cloned(),
            Some(TlsDefaultCertificateBindingSnapshot {
                listener_name: listener.name.clone(),
                server_name: server_name.clone(),
                certificate_scopes: binding.certificate_scopes.clone(),
                fingerprints: binding.fingerprints.clone(),
            }),
        );
    }

    (None, None)
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

#[cfg(test)]
mod tests;
