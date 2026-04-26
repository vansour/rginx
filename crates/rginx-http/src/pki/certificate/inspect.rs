use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::decode::{decode_certificate, load_certificate_chain_der};
use super::extensions::{
    authority_key_identifier, basic_constraints, describe_key_usage, extended_key_usage, key_usage,
    subject_alt_dns_names, subject_key_identifier,
};
use super::helpers::{
    fingerprint_sha256, integer_to_serial_string, integer_to_u32, time_to_unix_ms,
    time_to_unix_secs,
};
use super::name::name_to_string;
use super::{InspectedCertificate, TLS_EXPIRY_WARNING_DAYS};

pub(crate) fn inspect_certificate(path: &Path) -> Option<InspectedCertificate> {
    let certs = load_certificate_chain_der(path).ok()?;
    if certs.is_empty() {
        return None;
    }

    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let mut chain_subjects = Vec::new();
    let mut chain_entries = Vec::new();
    let mut chain_diagnostics = Vec::new();
    let mut seen_fingerprints = HashSet::new();

    for (index, der) in certs.iter().enumerate() {
        let fingerprint_sha256 = fingerprint_sha256(der.as_ref());
        if !seen_fingerprints.insert(fingerprint_sha256.clone()) {
            chain_diagnostics.push(format!(
                "duplicate_certificate_in_chain cert[{index}] sha256={fingerprint_sha256}"
            ));
        }

        let Some(cert) = decode_certificate(der.as_ref()) else {
            chain_diagnostics.push(format!("cert[{index}] could_not_be_parsed_as_x509"));
            continue;
        };

        let extensions = cert.tbs_certificate.extensions.as_ref().map(|value| value.as_slice());
        let subject = name_to_string(&cert.tbs_certificate.subject);
        let issuer = name_to_string(&cert.tbs_certificate.issuer);
        let expires_in_days = time_to_unix_secs(cert.tbs_certificate.validity.not_after)
            .map(|not_after| (not_after - now_secs).div_euclid(86_400));
        let basic_constraints = basic_constraints(extensions);
        let key_usage = key_usage(extensions);
        let extended_key_usage = extended_key_usage(extensions);
        let subject_key_identifier = subject_key_identifier(extensions);
        let authority_key_identifier = authority_key_identifier(extensions);

        if let Some(expires_in_days) = expires_in_days {
            if expires_in_days < 0 {
                chain_diagnostics.push(format!("cert[{index}] expired"));
            } else if expires_in_days <= TLS_EXPIRY_WARNING_DAYS {
                chain_diagnostics.push(format!("cert[{index}] expires_in_{expires_in_days}d"));
            }
        }
        if index == 0 && basic_constraints.as_ref().is_some_and(|constraints| constraints.ca) {
            chain_diagnostics.push("leaf_certificate_is_marked_as_ca".to_string());
        }
        if index == 0
            && key_usage.as_ref().is_some_and(|usage| {
                !usage.digital_signature && !usage.key_encipherment && !usage.key_agreement
            })
        {
            chain_diagnostics.push("leaf_key_usage_may_not_allow_tls_server_auth".to_string());
        }
        if index == 0
            && extended_key_usage.as_ref().is_some_and(|usage| {
                !usage.iter().any(|value| value == "any" || value == "server_auth")
            })
        {
            chain_diagnostics.push("leaf_missing_server_auth_eku".to_string());
        }
        if index > 0 && !basic_constraints.as_ref().is_some_and(|constraints| constraints.ca) {
            chain_diagnostics.push(format!("cert[{index}] intermediate_or_root_not_marked_as_ca"));
        }
        if index > 0 && key_usage.as_ref().is_some_and(|usage| !usage.key_cert_sign) {
            chain_diagnostics
                .push(format!("cert[{index}] intermediate_or_root_missing_key_cert_sign"));
        }

        chain_subjects.push(subject.clone());
        chain_entries.push(InspectedCertificate {
            subject: Some(subject),
            issuer: Some(issuer),
            serial_number: Some(integer_to_serial_string(&cert.tbs_certificate.serial_number)),
            san_dns_names: subject_alt_dns_names(extensions),
            fingerprint_sha256: Some(fingerprint_sha256),
            subject_key_identifier,
            authority_key_identifier,
            is_ca: basic_constraints.as_ref().map(|constraints| constraints.ca),
            path_len_constraint: basic_constraints
                .as_ref()
                .and_then(|constraints| constraints.path_len_constraint.clone())
                .as_ref()
                .and_then(integer_to_u32),
            key_usage: key_usage.as_ref().map(describe_key_usage),
            extended_key_usage: extended_key_usage.unwrap_or_default(),
            not_before_unix_ms: time_to_unix_ms(cert.tbs_certificate.validity.not_before),
            not_after_unix_ms: time_to_unix_ms(cert.tbs_certificate.validity.not_after),
            expires_in_days,
            chain_length: certs.len(),
            chain_subjects: Vec::new(),
            chain_diagnostics: Vec::new(),
        });
    }

    if chain_entries.len() == certs.len() {
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
                let descendant_ca_certs = chain_entries[..index + 1]
                    .iter()
                    .filter(|entry| entry.is_ca == Some(true))
                    .count() as u32;
                if descendant_ca_certs > path_len_constraint {
                    chain_diagnostics.push(format!(
                        "cert[{}] path_len_constraint_exceeded descendant_ca_certs={} path_len_constraint={}",
                        index + 1,
                        descendant_ca_certs,
                        path_len_constraint
                    ));
                }
            }
        }
    } else if certs.len() > 1 {
        chain_diagnostics
            .push("chain_link_checks_skipped_due_to_unparseable_certificate".to_string());
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
