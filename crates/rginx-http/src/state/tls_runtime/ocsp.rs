use super::*;

#[derive(Debug, Clone)]
struct TlsOcspBundleSpec {
    scope: String,
    cert_path: PathBuf,
    ocsp_staple_path: Option<PathBuf>,
    ocsp: rginx_core::OcspConfig,
}

pub fn tls_ocsp_refresh_specs_for_config(config: &ConfigSnapshot) -> Vec<TlsOcspRefreshSpec> {
    let mut specs = tls_ocsp_bundle_specs(config)
        .into_iter()
        .filter_map(|bundle| build_tls_ocsp_refresh_spec(&bundle))
        .collect::<Vec<_>>();
    specs.sort_by(|left, right| left.scope.cmp(&right.scope));
    specs
}

pub(super) fn tls_ocsp_status_snapshots(
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
                ocsp: tls.ocsp.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("listener:{}/additional[{index}]", listener.name),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                    ocsp: bundle.ocsp.clone(),
                },
            ));
        }
    }

    if let Some(tls) = config.default_vhost.tls.as_ref() {
        bundles.push(TlsOcspBundleSpec {
            scope: format!("vhost:{}", config.default_vhost.id),
            cert_path: tls.cert_path.clone(),
            ocsp_staple_path: tls.ocsp_staple_path.clone(),
            ocsp: tls.ocsp.clone(),
        });
        bundles.extend(tls.additional_certificates.iter().enumerate().map(|(index, bundle)| {
            TlsOcspBundleSpec {
                scope: format!("vhost:{}/additional[{index}]", config.default_vhost.id),
                cert_path: bundle.cert_path.clone(),
                ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                ocsp: bundle.ocsp.clone(),
            }
        }));
    }

    for vhost in &config.vhosts {
        if let Some(tls) = vhost.tls.as_ref() {
            bundles.push(TlsOcspBundleSpec {
                scope: format!("vhost:{}", vhost.id),
                cert_path: tls.cert_path.clone(),
                ocsp_staple_path: tls.ocsp_staple_path.clone(),
                ocsp: tls.ocsp.clone(),
            });
            bundles.extend(tls.additional_certificates.iter().enumerate().map(
                |(index, bundle)| TlsOcspBundleSpec {
                    scope: format!("vhost:{}/additional[{index}]", vhost.id),
                    cert_path: bundle.cert_path.clone(),
                    ocsp_staple_path: bundle.ocsp_staple_path.clone(),
                    ocsp: bundle.ocsp.clone(),
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
    let refresh_spec = build_tls_ocsp_refresh_spec(bundle)?;
    let responder_error =
        ocsp_responder_urls_for_certificate(&bundle.cert_path).err().map(|error| error.to_string());
    let request_error = if bundle.ocsp_staple_path.is_some()
        && !refresh_spec.responder_urls.is_empty()
    {
        crate::build_ocsp_request_for_certificate_with_options(&bundle.cert_path, bundle.ocsp.nonce)
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };

    let (cache_loaded, cache_size_bytes, cache_modified_unix_ms, cache_error) = bundle
        .ocsp_staple_path
        .as_ref()
        .map(|path| inspect_ocsp_cache_file(&bundle.cert_path, path, bundle.ocsp.responder_policy))
        .unwrap_or((false, None, None, None));
    let runtime = runtime_statuses.and_then(|statuses| statuses.get(&bundle.scope));
    let static_error = cache_error.or(responder_error).or_else(|| {
        if bundle.ocsp_staple_path.is_some() && refresh_spec.responder_urls.is_empty() {
            Some("certificate does not expose an OCSP responder URL".to_string())
        } else {
            request_error
        }
    });

    Some(TlsOcspStatusSnapshot {
        scope: bundle.scope.clone(),
        cert_path: bundle.cert_path.clone(),
        ocsp_staple_path: bundle.ocsp_staple_path.clone(),
        responder_urls: refresh_spec.responder_urls,
        nonce_mode: match bundle.ocsp.nonce {
            rginx_core::OcspNonceMode::Disabled => "disabled".to_string(),
            rginx_core::OcspNonceMode::Preferred => "preferred".to_string(),
            rginx_core::OcspNonceMode::Required => "required".to_string(),
        },
        responder_policy: match bundle.ocsp.responder_policy {
            rginx_core::OcspResponderPolicy::IssuerOnly => "issuer_only".to_string(),
            rginx_core::OcspResponderPolicy::IssuerOrDelegated => "issuer_or_delegated".to_string(),
        },
        cache_loaded,
        cache_size_bytes,
        cache_modified_unix_ms,
        auto_refresh_enabled: refresh_spec.auto_refresh_enabled,
        last_refresh_unix_ms: runtime.and_then(|entry| entry.last_refresh_unix_ms),
        refreshes_total: runtime.map(|entry| entry.refreshes_total).unwrap_or(0),
        failures_total: runtime.map(|entry| entry.failures_total).unwrap_or(0),
        last_error: runtime.and_then(|entry| entry.last_error.clone()).or(static_error),
    })
}

fn build_tls_ocsp_refresh_spec(bundle: &TlsOcspBundleSpec) -> Option<TlsOcspRefreshSpec> {
    let (responder_urls, responder_error) =
        match ocsp_responder_urls_for_certificate(&bundle.cert_path) {
            Ok(responder_urls) => (responder_urls, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
    if bundle.ocsp_staple_path.is_none() && responder_urls.is_empty() && responder_error.is_none() {
        return None;
    }

    let request_error = if bundle.ocsp_staple_path.is_some() && !responder_urls.is_empty() {
        crate::build_ocsp_request_for_certificate_with_options(&bundle.cert_path, bundle.ocsp.nonce)
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    let auto_refresh_enabled = bundle.ocsp_staple_path.is_some()
        && !responder_urls.is_empty()
        && responder_error.is_none()
        && request_error.is_none();

    Some(TlsOcspRefreshSpec {
        scope: bundle.scope.clone(),
        cert_path: bundle.cert_path.clone(),
        ocsp_staple_path: bundle.ocsp_staple_path.clone(),
        responder_urls,
        auto_refresh_enabled,
        ocsp_nonce_mode: bundle.ocsp.nonce,
        ocsp_responder_policy: bundle.ocsp.responder_policy,
    })
}

fn inspect_ocsp_cache_file(
    cert_path: &std::path::Path,
    path: &PathBuf,
    responder_policy: rginx_core::OcspResponderPolicy,
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
        Ok(bytes) => crate::validate_ocsp_response_for_certificate_with_options(
            cert_path,
            &bytes,
            None,
            rginx_core::OcspNonceMode::Disabled,
            responder_policy,
        )
        .err()
        .map(|error| error.to_string()),
        Err(error) => Some(format!("failed to read OCSP cache file `{}`: {error}", path.display())),
    };
    (cache_error.is_none(), Some(size_bytes), modified, cache_error)
}
