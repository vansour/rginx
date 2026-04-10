use super::*;
use crate::config_transition_boundary;

pub fn tls_runtime_snapshot_for_config(config: &ConfigSnapshot) -> TlsRuntimeSnapshot {
    tls_runtime_snapshot_for_config_with_ocsp_statuses(config, None)
}

pub(super) fn tls_runtime_snapshot_for_config_with_ocsp_statuses(
    config: &ConfigSnapshot,
    ocsp_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> TlsRuntimeSnapshot {
    let listeners = config
        .listeners
        .iter()
        .map(|listener| {
            let sni_names = tls_listener_sni_names(config, listener.tls_enabled());
            let tls = listener.server.tls.as_ref();
            TlsListenerStatusSnapshot {
                listener_id: listener.id.clone(),
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                tls_enabled: listener.tls_enabled(),
                default_certificate: listener.server.default_certificate.clone(),
                versions: tls.and_then(|tls| {
                    tls.versions.as_ref().map(|versions| {
                        versions
                            .iter()
                            .map(|version| tls_version_label(*version).to_string())
                            .collect()
                    })
                }),
                alpn_protocols: tls
                    .and_then(|tls| tls.alpn_protocols.clone())
                    .unwrap_or_else(|| vec!["h2".to_string(), "http/1.1".to_string()]),
                session_resumption_enabled: tls.map(|tls| tls.session_resumption != Some(false)),
                session_tickets_enabled: tls.map(|tls| {
                    tls.session_resumption != Some(false) && tls.session_tickets != Some(false)
                }),
                session_cache_size: tls.map(|tls| {
                    if tls.session_resumption == Some(false) {
                        0
                    } else {
                        tls.session_cache_size.unwrap_or(256)
                    }
                }),
                session_ticket_count: tls.map(|tls| {
                    if tls.session_resumption == Some(false) || tls.session_tickets == Some(false) {
                        0
                    } else {
                        tls.session_ticket_count.unwrap_or(2)
                    }
                }),
                client_auth_mode: tls.and_then(|tls| {
                    tls.client_auth.as_ref().map(|client_auth| match client_auth.mode {
                        rginx_core::ServerClientAuthMode::Optional => "optional".to_string(),
                        rginx_core::ServerClientAuthMode::Required => "required".to_string(),
                    })
                }),
                client_auth_verify_depth: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.verify_depth),
                client_auth_crl_configured: tls
                    .and_then(|tls| tls.client_auth.as_ref())
                    .and_then(|client_auth| client_auth.crl_path.as_ref())
                    .is_some(),
                sni_names,
            }
        })
        .collect::<Vec<_>>();

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

pub fn tls_reloadable_fields() -> Vec<String> {
    config_transition_boundary().reloadable_fields
}

pub fn tls_restart_required_fields() -> Vec<String> {
    config_transition_boundary().restart_required_fields
}

fn tls_listener_sni_names(config: &ConfigSnapshot, listener_has_tls: bool) -> Vec<String> {
    if !listener_has_tls && !config.vhosts.iter().any(|vhost| vhost.tls.is_some()) {
        return Vec::new();
    }

    let mut names = config.default_vhost.server_names.clone();
    for vhost in &config.vhosts {
        if vhost.tls.is_some() {
            names.extend(vhost.server_names.clone());
        }
    }
    names.sort();
    names.dedup();
    names
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
        selected_as_default_for_listeners: if listener.server.default_certificate.is_none()
            || listener.server.default_certificate.as_ref().is_some_and(|default_name| {
                config.default_vhost.server_names.iter().any(|name| name == default_name)
            }) {
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

fn tls_binding_snapshots(
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

#[derive(Debug, Clone)]
struct TlsOcspBundleSpec {
    scope: String,
    cert_path: PathBuf,
    ocsp_staple_path: Option<PathBuf>,
}

fn tls_ocsp_status_snapshots(
    config: &ConfigSnapshot,
    runtime_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> Vec<TlsOcspStatusSnapshot> {
    let mut statuses = tls_ocsp_bundle_specs(config)
        .into_iter()
        .filter_map(|bundle| build_tls_ocsp_status_snapshot(&bundle, runtime_statuses))
        .collect::<Vec<_>>();
    statuses.sort_by(|left, right| left.scope.cmp(&right.scope));
    statuses
}

fn tls_ocsp_bundle_specs(config: &ConfigSnapshot) -> Vec<TlsOcspBundleSpec> {
    let mut bundles = Vec::new();
    for listener in &config.listeners {
        if let Some(tls) = listener.server.tls.as_ref() {
            bundles.push(TlsOcspBundleSpec {
                scope: format!("listener:{}", listener.name),
                cert_path: tls.cert_path.clone(),
                ocsp_staple_path: tls.ocsp_staple_path.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("listener:{}/additional[{index}]", listener.name),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                },
            ));
        }
    }

    if let Some(tls) = config.default_vhost.tls.as_ref() {
        bundles.push(TlsOcspBundleSpec {
            scope: format!("vhost:{}", config.default_vhost.id),
            cert_path: tls.cert_path.clone(),
            ocsp_staple_path: tls.ocsp_staple_path.clone(),
        });
        bundles.extend(tls.additional_certificates.iter().enumerate().map(|(index, bundle)| {
            TlsOcspBundleSpec {
                scope: format!("vhost:{}/additional[{index}]", config.default_vhost.id),
                cert_path: bundle.cert_path.clone(),
                ocsp_staple_path: bundle.ocsp_staple_path.clone(),
            }
        }));
    }

    for vhost in &config.vhosts {
        if let Some(tls) = vhost.tls.as_ref() {
            bundles.push(TlsOcspBundleSpec {
                scope: format!("vhost:{}", vhost.id),
                cert_path: tls.cert_path.clone(),
                ocsp_staple_path: tls.ocsp_staple_path.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("vhost:{}/additional[{index}]", vhost.id),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                },
            ));
        }
    }

    bundles
}

fn build_tls_ocsp_status_snapshot(
    bundle: &TlsOcspBundleSpec,
    runtime_statuses: Option<&HashMap<String, OcspRuntimeStatusEntry>>,
) -> Option<TlsOcspStatusSnapshot> {
    let (responder_urls, responder_error) =
        match ocsp_responder_urls_for_certificate(&bundle.cert_path) {
            Ok(responder_urls) => (responder_urls, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
    if bundle.ocsp_staple_path.is_none() && responder_urls.is_empty() && responder_error.is_none() {
        return None;
    }

    let (cache_loaded, cache_size_bytes, cache_modified_unix_ms, cache_error) = bundle
        .ocsp_staple_path
        .as_ref()
        .map(|path| inspect_ocsp_cache_file(&bundle.cert_path, path))
        .unwrap_or((false, None, None, None));
    let runtime = runtime_statuses.and_then(|statuses| statuses.get(&bundle.scope));
    let ocsp_request_result = if bundle.ocsp_staple_path.is_some() && !responder_urls.is_empty() {
        Some(crate::build_ocsp_request_for_certificate(&bundle.cert_path))
    } else {
        None
    };
    let request_error = ocsp_request_result
        .as_ref()
        .and_then(|result| result.as_ref().err().map(|error| error.to_string()));
    let auto_refresh_enabled = bundle.ocsp_staple_path.is_some()
        && !responder_urls.is_empty()
        && responder_error.is_none()
        && request_error.is_none();
    let static_error = cache_error.or(responder_error).or_else(|| {
        if bundle.ocsp_staple_path.is_some() && responder_urls.is_empty() {
            Some("certificate does not expose an OCSP responder URL".to_string())
        } else {
            request_error
        }
    });

    Some(TlsOcspStatusSnapshot {
        scope: bundle.scope.clone(),
        cert_path: bundle.cert_path.clone(),
        ocsp_staple_path: bundle.ocsp_staple_path.clone(),
        responder_urls,
        cache_loaded,
        cache_size_bytes,
        cache_modified_unix_ms,
        auto_refresh_enabled,
        last_refresh_unix_ms: runtime.and_then(|entry| entry.last_refresh_unix_ms),
        refreshes_total: runtime.map(|entry| entry.refreshes_total).unwrap_or(0),
        failures_total: runtime.map(|entry| entry.failures_total).unwrap_or(0),
        last_error: runtime.and_then(|entry| entry.last_error.clone()).or(static_error),
    })
}

fn inspect_ocsp_cache_file(
    cert_path: &std::path::Path,
    path: &PathBuf,
) -> (bool, Option<usize>, Option<u64>, Option<String>) {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (false, None, None, None);
        }
        Err(error) => {
            return (
                false,
                None,
                None,
                Some(format!("failed to stat OCSP cache file `{}`: {error}", path.display())),
            );
        }
    };
    let size = usize::try_from(metadata.len()).ok();
    let modified = metadata.modified().ok().map(unix_time_ms);
    let Some(size_bytes) = size else {
        return (
            false,
            size,
            modified,
            Some("OCSP cache file size exceeds platform limits".to_string()),
        );
    };
    if size_bytes == 0 {
        return (false, Some(0), modified, None);
    }
    if size_bytes > crate::MAX_OCSP_RESPONSE_BYTES {
        return (
            false,
            Some(size_bytes),
            modified,
            Some(format!("OCSP cache file exceeds {} bytes", crate::MAX_OCSP_RESPONSE_BYTES)),
        );
    }

    let cache_error = match std::fs::File::open(path).and_then(|file| {
        use std::io::Read;

        let mut reader = file.take(crate::MAX_OCSP_RESPONSE_BYTES as u64 + 1);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }) {
        Ok(bytes) if bytes.len() > crate::MAX_OCSP_RESPONSE_BYTES => {
            Some(format!("OCSP cache file exceeds {} bytes", crate::MAX_OCSP_RESPONSE_BYTES))
        }
        Ok(bytes) => validate_ocsp_response_for_certificate(cert_path, &bytes)
            .err()
            .map(|error| error.to_string()),
        Err(error) => Some(format!("failed to read OCSP cache file `{}`: {error}", path.display())),
    };
    (cache_error.is_none(), Some(size_bytes), modified, cache_error)
}

#[derive(Debug, Clone)]
pub(crate) struct InspectedCertificate {
    pub(crate) subject: Option<String>,
    pub(crate) issuer: Option<String>,
    pub(crate) serial_number: Option<String>,
    pub(crate) san_dns_names: Vec<String>,
    pub(crate) fingerprint_sha256: Option<String>,
    pub(crate) subject_key_identifier: Option<String>,
    pub(crate) authority_key_identifier: Option<String>,
    pub(crate) is_ca: Option<bool>,
    pub(crate) path_len_constraint: Option<u32>,
    pub(crate) key_usage: Option<String>,
    pub(crate) extended_key_usage: Vec<String>,
    pub(crate) not_before_unix_ms: Option<u64>,
    pub(crate) not_after_unix_ms: Option<u64>,
    pub(crate) expires_in_days: Option<i64>,
    pub(crate) chain_length: usize,
    pub(crate) chain_subjects: Vec<String>,
    pub(crate) chain_diagnostics: Vec<String>,
}

pub(crate) fn inspect_certificate(path: &std::path::Path) -> Option<InspectedCertificate> {
    let certs = load_certificate_chain_der(path).ok()?;
    if certs.is_empty() {
        return None;
    }

    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let mut chain_subjects = Vec::new();
    let mut chain_entries = Vec::new();
    let mut chain_diagnostics = Vec::new();
    let mut seen_fingerprints = std::collections::HashSet::new();

    for (index, der) in certs.iter().enumerate() {
        let fingerprint_sha256 = fingerprint_sha256(der.as_ref());
        if !seen_fingerprints.insert(fingerprint_sha256.clone()) {
            chain_diagnostics.push(format!(
                "duplicate_certificate_in_chain cert[{index}] sha256={fingerprint_sha256}"
            ));
        }

        match X509Certificate::from_der(der.as_ref()) {
            Ok((_, cert)) => {
                let subject = format!("{}", cert.subject());
                let issuer = format!("{}", cert.issuer());
                let expires_in_days = (cert.validity().not_after.timestamp() - now_secs) / 86_400;
                let basic_constraints = cert.basic_constraints().ok().flatten();
                let key_usage = cert.key_usage().ok().flatten();
                let extended_key_usage = cert.extended_key_usage().ok().flatten();
                let subject_key_identifier = extension_key_identifier(&cert, true);
                let authority_key_identifier = extension_key_identifier(&cert, false);
                if cert.validity().not_after.timestamp() < now_secs {
                    chain_diagnostics.push(format!("cert[{index}] expired"));
                } else if expires_in_days <= TLS_EXPIRY_WARNING_DAYS {
                    chain_diagnostics.push(format!("cert[{index}] expires_in_{expires_in_days}d"));
                }
                if index == 0 && cert.is_ca() {
                    chain_diagnostics.push("leaf_certificate_is_marked_as_ca".to_string());
                }
                if index == 0
                    && key_usage.as_ref().is_some_and(|extension| {
                        !extension.value.digital_signature()
                            && !extension.value.key_encipherment()
                            && !extension.value.key_agreement()
                    })
                {
                    chain_diagnostics
                        .push("leaf_key_usage_may_not_allow_tls_server_auth".to_string());
                }
                if index == 0
                    && extended_key_usage.as_ref().is_some_and(|extension| {
                        !extension.value.any && !extension.value.server_auth
                    })
                {
                    chain_diagnostics.push("leaf_missing_server_auth_eku".to_string());
                }
                if index > 0
                    && !basic_constraints.as_ref().is_some_and(|extension| extension.value.ca)
                {
                    chain_diagnostics
                        .push(format!("cert[{index}] intermediate_or_root_not_marked_as_ca"));
                }
                if index > 0
                    && key_usage.as_ref().is_some_and(|extension| !extension.value.key_cert_sign())
                {
                    chain_diagnostics
                        .push(format!("cert[{index}] intermediate_or_root_missing_key_cert_sign"));
                }
                chain_subjects.push(subject.clone());
                chain_entries.push(InspectedCertificate {
                    subject: Some(subject),
                    issuer: Some(issuer),
                    serial_number: Some(cert.tbs_certificate.raw_serial_as_string()),
                    san_dns_names: cert
                        .subject_alternative_name()
                        .ok()
                        .flatten()
                        .map(|san| {
                            san.value
                                .general_names
                                .iter()
                                .filter_map(|name| match name {
                                    GeneralName::DNSName(dns) => Some(dns.to_string()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    fingerprint_sha256: Some(fingerprint_sha256),
                    subject_key_identifier,
                    authority_key_identifier,
                    is_ca: basic_constraints.as_ref().map(|extension| extension.value.ca),
                    path_len_constraint: basic_constraints
                        .as_ref()
                        .and_then(|extension| extension.value.path_len_constraint),
                    key_usage: key_usage.as_ref().map(|extension| extension.value.to_string()),
                    extended_key_usage: describe_extended_key_usage(extended_key_usage.as_ref()),
                    not_before_unix_ms: cert
                        .validity()
                        .not_before
                        .timestamp()
                        .checked_mul(1000)
                        .and_then(|timestamp| timestamp.try_into().ok()),
                    not_after_unix_ms: cert
                        .validity()
                        .not_after
                        .timestamp()
                        .checked_mul(1000)
                        .and_then(|timestamp| timestamp.try_into().ok()),
                    expires_in_days: Some(expires_in_days),
                    chain_length: certs.len(),
                    chain_subjects: Vec::new(),
                    chain_diagnostics: Vec::new(),
                });
            }
            Err(_) => {
                chain_diagnostics.push(format!("cert[{index}] could_not_be_parsed_as_x509"));
            }
        }
    }

    for index in 0..chain_entries.len().saturating_sub(1) {
        let issuer = chain_entries[index].issuer.as_deref();
        let next_subject = chain_entries[index + 1].subject.as_deref();
        if issuer != next_subject {
            chain_diagnostics.push(format!(
                "chain_link_mismatch cert[{index}]_issuer_to_cert[{}]_subject",
                index + 1
            ));
        }
        if let (Some(aki), Some(ski)) = (
            chain_entries[index].authority_key_identifier.as_deref(),
            chain_entries[index + 1].subject_key_identifier.as_deref(),
        ) && aki != ski
        {
            chain_diagnostics
                .push(format!("chain_aki_ski_mismatch cert[{index}]_to_cert[{}]", index + 1));
        }
        if let Some(path_len_constraint) = chain_entries[index + 1].path_len_constraint {
            let remaining_ca_certs =
                chain_entries[index + 2..].iter().filter(|entry| entry.is_ca == Some(true)).count()
                    as u32;
            if remaining_ca_certs > path_len_constraint {
                chain_diagnostics.push(format!(
                    "cert[{}] path_len_constraint_exceeded remaining_ca_certs={} path_len_constraint={}",
                    index + 1,
                    remaining_ca_certs,
                    path_len_constraint
                ));
            }
        }
    }

    if let Some(leaf) = chain_entries.first() {
        if certs.len() == 1 {
            if leaf.subject != leaf.issuer {
                chain_diagnostics
                    .push("chain_incomplete_single_non_self_signed_certificate".to_string());
            }
        } else if let Some(last) = chain_entries.last()
            && last.subject != last.issuer
        {
            chain_diagnostics.push("chain_incomplete_non_self_signed_top_certificate".to_string());
        }
    }

    let leaf = chain_entries.into_iter().next()?;
    Some(InspectedCertificate {
        chain_length: certs.len(),
        chain_subjects,
        chain_diagnostics,
        ..leaf
    })
}

fn load_certificate_chain_der(path: &std::path::Path) -> std::io::Result<Vec<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if !certs.is_empty() {
        return Ok(certs.into_iter().map(|cert| cert.as_ref().to_vec()).collect());
    }
    Ok(vec![std::fs::read(path)?])
}

fn fingerprint_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

fn extension_key_identifier(cert: &X509Certificate<'_>, subject: bool) -> Option<String> {
    cert.iter_extensions().find_map(|extension| match extension.parsed_extension() {
        ParsedExtension::SubjectKeyIdentifier(identifier) if subject => {
            Some(format!("{identifier:x}"))
        }
        ParsedExtension::AuthorityKeyIdentifier(identifier) if !subject => {
            identifier.key_identifier.as_ref().map(|identifier| format!("{identifier:x}"))
        }
        _ => None,
    })
}

fn describe_extended_key_usage(
    extension: Option<
        &x509_parser::certificate::BasicExtension<&x509_parser::extensions::ExtendedKeyUsage<'_>>,
    >,
) -> Vec<String> {
    let Some(extension) = extension else {
        return Vec::new();
    };

    let mut usages = Vec::new();
    if extension.value.any {
        usages.push("any".to_string());
    }
    if extension.value.server_auth {
        usages.push("server_auth".to_string());
    }
    if extension.value.client_auth {
        usages.push("client_auth".to_string());
    }
    if extension.value.code_signing {
        usages.push("code_signing".to_string());
    }
    if extension.value.email_protection {
        usages.push("email_protection".to_string());
    }
    if extension.value.time_stamping {
        usages.push("time_stamping".to_string());
    }
    if extension.value.ocsp_signing {
        usages.push("ocsp_signing".to_string());
    }
    usages.extend(extension.value.other.iter().map(|oid| oid.to_id_string()));
    usages
}

fn tls_version_label(version: rginx_core::TlsVersion) -> &'static str {
    match version {
        rginx_core::TlsVersion::Tls12 => "TLS1.2",
        rginx_core::TlsVersion::Tls13 => "TLS1.3",
    }
}
