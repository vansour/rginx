use super::*;

use crate::pki::inspect_certificate;

pub(super) fn tls_certificate_status_snapshots(
    config: &ConfigSnapshot,
) -> Vec<TlsCertificateStatusSnapshot> {
    let mut certificates = Vec::new();
    for listener in &config.listeners {
        if let Some(tls) = listener.server.tls.as_ref() {
            certificates.push(build_listener_certificate_snapshot(config, listener, tls));
        }
    }
    if let Some(snapshot) = build_vhost_certificate_snapshot(config, &config.default_vhost) {
        certificates.push(snapshot);
    }
    certificates.extend(
        config.vhosts.iter().filter_map(|vhost| build_vhost_certificate_snapshot(config, vhost)),
    );
    certificates
}

fn build_listener_certificate_snapshot(
    config: &ConfigSnapshot,
    listener: &Listener,
    tls: &rginx_core::ServerTls,
) -> TlsCertificateStatusSnapshot {
    let inspected = inspect_certificate(&tls.cert_path);
    TlsCertificateStatusSnapshot {
        scope: format!("listener:{}", listener.name),
        cert_path: tls.cert_path.clone(),
        server_names: config.default_vhost.server_names.clone(),
        subject: inspected.as_ref().and_then(|certificate| certificate.subject.clone()),
        issuer: inspected.as_ref().and_then(|certificate| certificate.issuer.clone()),
        serial_number: inspected.as_ref().and_then(|certificate| certificate.serial_number.clone()),
        san_dns_names: inspected
            .as_ref()
            .map(|certificate| certificate.san_dns_names.clone())
            .unwrap_or_default(),
        fingerprint_sha256: inspected
            .as_ref()
            .and_then(|certificate| certificate.fingerprint_sha256.clone()),
        subject_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.subject_key_identifier.clone()),
        authority_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.authority_key_identifier.clone()),
        is_ca: inspected.as_ref().and_then(|certificate| certificate.is_ca),
        path_len_constraint: inspected
            .as_ref()
            .and_then(|certificate| certificate.path_len_constraint),
        key_usage: inspected.as_ref().and_then(|certificate| certificate.key_usage.clone()),
        extended_key_usage: inspected
            .as_ref()
            .map(|certificate| certificate.extended_key_usage.clone())
            .unwrap_or_default(),
        not_before_unix_ms: inspected
            .as_ref()
            .and_then(|certificate| certificate.not_before_unix_ms),
        not_after_unix_ms: inspected.as_ref().and_then(|certificate| certificate.not_after_unix_ms),
        expires_in_days: inspected.as_ref().and_then(|certificate| certificate.expires_in_days),
        chain_length: inspected.as_ref().map(|certificate| certificate.chain_length).unwrap_or(0),
        chain_subjects: inspected
            .as_ref()
            .map(|certificate| certificate.chain_subjects.clone())
            .unwrap_or_default(),
        chain_diagnostics: inspected
            .as_ref()
            .map(|certificate| certificate.chain_diagnostics.clone())
            .unwrap_or_default(),
        selected_as_default_for_listeners: if listener.server.default_certificate.is_none() {
            vec![listener.name.clone()]
        } else {
            Vec::new()
        },
        ocsp_staple_configured: tls.ocsp_staple_path.is_some(),
        additional_certificate_count: tls.additional_certificates.len(),
    }
}

fn build_vhost_certificate_snapshot(
    config: &ConfigSnapshot,
    vhost: &rginx_core::VirtualHost,
) -> Option<TlsCertificateStatusSnapshot> {
    let tls = vhost.tls.as_ref()?;
    let inspected = inspect_certificate(&tls.cert_path);
    Some(TlsCertificateStatusSnapshot {
        scope: format!("vhost:{}", vhost.id),
        cert_path: tls.cert_path.clone(),
        server_names: vhost.server_names.clone(),
        subject: inspected.as_ref().and_then(|certificate| certificate.subject.clone()),
        issuer: inspected.as_ref().and_then(|certificate| certificate.issuer.clone()),
        serial_number: inspected.as_ref().and_then(|certificate| certificate.serial_number.clone()),
        san_dns_names: inspected
            .as_ref()
            .map(|certificate| certificate.san_dns_names.clone())
            .unwrap_or_default(),
        fingerprint_sha256: inspected
            .as_ref()
            .and_then(|certificate| certificate.fingerprint_sha256.clone()),
        subject_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.subject_key_identifier.clone()),
        authority_key_identifier: inspected
            .as_ref()
            .and_then(|certificate| certificate.authority_key_identifier.clone()),
        is_ca: inspected.as_ref().and_then(|certificate| certificate.is_ca),
        path_len_constraint: inspected
            .as_ref()
            .and_then(|certificate| certificate.path_len_constraint),
        key_usage: inspected.as_ref().and_then(|certificate| certificate.key_usage.clone()),
        extended_key_usage: inspected
            .as_ref()
            .map(|certificate| certificate.extended_key_usage.clone())
            .unwrap_or_default(),
        not_before_unix_ms: inspected
            .as_ref()
            .and_then(|certificate| certificate.not_before_unix_ms),
        not_after_unix_ms: inspected.as_ref().and_then(|certificate| certificate.not_after_unix_ms),
        expires_in_days: inspected.as_ref().and_then(|certificate| certificate.expires_in_days),
        chain_length: inspected.as_ref().map(|certificate| certificate.chain_length).unwrap_or(0),
        chain_subjects: inspected
            .as_ref()
            .map(|certificate| certificate.chain_subjects.clone())
            .unwrap_or_default(),
        chain_diagnostics: inspected
            .as_ref()
            .map(|certificate| certificate.chain_diagnostics.clone())
            .unwrap_or_default(),
        selected_as_default_for_listeners: config
            .listeners
            .iter()
            .filter_map(|listener| {
                listener
                    .server
                    .default_certificate
                    .as_ref()
                    .filter(|default_name| {
                        vhost.server_names.iter().any(|name| name == *default_name)
                    })
                    .map(|_| listener.name.clone())
            })
            .collect(),
        ocsp_staple_configured: tls.ocsp_staple_path.is_some(),
        additional_certificate_count: tls.additional_certificates.len(),
    })
}
