use super::super::summary::CheckSummary;
use super::render_string_list;

pub(super) fn print_tls_details(summary: &CheckSummary) {
    print_tls_overview(summary);
    print_tls_listeners(summary);
    print_tls_certificates(summary);
    print_tls_ocsp(summary);
    print_tls_bindings(summary);
}

fn print_tls_overview(summary: &CheckSummary) {
    // Keep both keys for backward-compatible machine parsing across older check consumers.
    println!("reload_requires_restart_for={}", summary.tls.restart_required_fields.join(","));
    println!(
        "tls_details=listener_profiles={} vhost_overrides={} sni_names={} certificate_bundles={}",
        summary.tls.listener_tls_profiles,
        summary.tls.vhost_tls_overrides,
        summary.tls.sni_name_count,
        summary.tls.certificate_bundle_count,
    );
    println!("reload_tls_updates={}", summary.tls.reloadable_fields.join(","));

    if summary.tls.default_certificates.is_empty() {
        println!("tls_default_certificates=-");
    } else {
        println!("tls_default_certificates={}", summary.tls.default_certificates.join(","));
    }

    if summary.tls.expiring_certificates.is_empty() {
        println!("tls_expiring_certificates=-");
    } else {
        println!("tls_expiring_certificates={}", summary.tls.expiring_certificates.join(","));
    }

    println!("tls_restart_required_fields={}", summary.tls.restart_required_fields.join(","));
}

fn print_tls_listeners(summary: &CheckSummary) {
    for listener in &summary.tls.listeners {
        println!(
            "tls_listener listener={} listener_id={} listen={} tls={} default_certificate={} tcp_versions={} tcp_alpn_protocols={} http3_enabled={} http3_listen={} http3_versions={} http3_alpn_protocols={} http3_max_concurrent_streams={} http3_stream_buffer_size={} http3_active_connection_id_limit={} http3_retry={} http3_host_key_path={} http3_gso={} http3_early_data_enabled={} sni_names={}",
            listener.listener_name,
            listener.listener_id,
            listener.listen_addr,
            listener.tls_enabled,
            listener.default_certificate.as_deref().unwrap_or("-"),
            listener
                .versions
                .as_ref()
                .filter(|versions| !versions.is_empty())
                .map(|versions| versions.join(","))
                .unwrap_or_else(|| "-".to_string()),
            render_string_list(&listener.alpn_protocols),
            listener.http3_enabled,
            listener
                .http3_listen_addr
                .map(|listen_addr| listen_addr.to_string())
                .unwrap_or_else(|| "-".to_string()),
            render_string_list(&listener.http3_versions),
            render_string_list(&listener.http3_alpn_protocols),
            listener
                .http3_max_concurrent_streams
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener
                .http3_stream_buffer_size
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener
                .http3_active_connection_id_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener.http3_retry.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            listener
                .http3_host_key_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            listener.http3_gso.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            listener
                .http3_early_data_enabled
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            render_string_list(&listener.sni_names),
        );
    }
}

fn print_tls_certificates(summary: &CheckSummary) {
    for certificate in &summary.tls.certificates {
        println!(
            "tls_certificate scope={} sha256={} subject={} issuer={} serial={} chain_length={} diagnostics={} cert_path={}",
            certificate.scope,
            certificate.fingerprint_sha256.as_deref().unwrap_or("-"),
            certificate.subject.as_deref().unwrap_or("-"),
            certificate.issuer.as_deref().unwrap_or("-"),
            certificate.serial_number.as_deref().unwrap_or("-"),
            certificate.chain_length,
            if certificate.chain_diagnostics.is_empty() {
                "-".to_string()
            } else {
                certificate.chain_diagnostics.join("|")
            },
            certificate.cert_path.display(),
        );
    }
}

fn print_tls_ocsp(summary: &CheckSummary) {
    for ocsp in &summary.tls.ocsp {
        println!(
            "tls_ocsp scope={} cert_path={} staple_path={} responder_urls={} nonce_mode={} responder_policy={} cache_loaded={} cache_size_bytes={} cache_modified_unix_ms={} auto_refresh_enabled={} last_refresh_unix_ms={} refreshes_total={} failures_total={} last_error={}",
            ocsp.scope,
            ocsp.cert_path.display(),
            ocsp.ocsp_staple_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            render_string_list(&ocsp.responder_urls),
            ocsp.nonce_mode,
            ocsp.responder_policy,
            ocsp.cache_loaded,
            ocsp.cache_size_bytes.map(|value| value.to_string()).unwrap_or_else(|| "-".to_string()),
            ocsp.cache_modified_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            ocsp.auto_refresh_enabled,
            ocsp.last_refresh_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            ocsp.refreshes_total,
            ocsp.failures_total,
            ocsp.last_error.as_deref().unwrap_or("-"),
        );
    }
}

fn print_tls_bindings(summary: &CheckSummary) {
    for binding in &summary.tls.vhost_bindings {
        println!(
            "tls_vhost_binding listener={} vhost={} server_names={} certificate_scopes={} fingerprints={} default_selected={}",
            binding.listener_name,
            binding.vhost_id,
            render_string_list(&binding.server_names),
            render_string_list(&binding.certificate_scopes),
            render_string_list(&binding.fingerprints),
            binding.default_selected,
        );
    }

    for binding in &summary.tls.sni_bindings {
        println!(
            "tls_sni_binding listener={} server_name={} fingerprints={} scopes={} default_selected={}",
            binding.listener_name,
            binding.server_name,
            render_string_list(&binding.fingerprints),
            render_string_list(&binding.scopes),
            binding.default_selected,
        );
    }

    if summary.tls.sni_conflicts.is_empty() {
        println!("tls_sni_conflicts=-");
    } else {
        for binding in &summary.tls.sni_conflicts {
            println!(
                "tls_sni_conflict listener={} server_name={} fingerprints={} scopes={}",
                binding.listener_name,
                binding.server_name,
                render_string_list(&binding.fingerprints),
                render_string_list(&binding.scopes),
            );
        }
    }

    for binding in &summary.tls.default_certificate_bindings {
        println!(
            "tls_default_certificate_binding listener={} server_name={} fingerprints={} scopes={}",
            binding.listener_name,
            binding.server_name,
            render_string_list(&binding.fingerprints),
            render_string_list(&binding.scopes),
        );
    }
}
