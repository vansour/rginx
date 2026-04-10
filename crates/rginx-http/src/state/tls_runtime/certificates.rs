use super::*;

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
