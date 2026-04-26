use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{
    ServerCertificateBundleConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig,
    TlsVersionConfig,
};

/// Validates TLS identity file paths and related certificate bundle fields.
pub(super) fn validate_tls_identity_fields(
    owner_label: &str,
    cert_path: &str,
    key_path: &str,
    additional_certificates: Option<&[ServerCertificateBundleConfig]>,
    ocsp_staple_path: Option<&str>,
) -> Result<()> {
    if cert_path.trim().is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS certificate path must not be empty")));
    }

    if key_path.trim().is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS private key path must not be empty")));
    }

    if ocsp_staple_path.is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!("{owner_label} TLS OCSP staple path must not be empty")));
    }

    if let Some(additional_certificates) = additional_certificates {
        if additional_certificates.is_empty() {
            return Err(Error::Config(format!(
                "{owner_label} TLS additional_certificates must not be empty"
            )));
        }

        for bundle in additional_certificates {
            validate_certificate_bundle(owner_label, bundle)?;
        }
    }

    Ok(())
}

/// Validates the configured TLS version list.
pub(super) fn validate_tls_versions(
    owner_label: &str,
    versions: Option<&[TlsVersionConfig]>,
) -> Result<()> {
    let Some(versions) = versions else {
        return Ok(());
    };

    if versions.is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS versions must not be empty")));
    }

    let mut seen = HashSet::new();
    for version in versions {
        if !seen.insert(version) {
            return Err(Error::Config(format!(
                "{owner_label} TLS versions must not contain duplicates"
            )));
        }
    }

    Ok(())
}

/// Validates configured TLS cipher suites against the allowed TLS versions.
pub(super) fn validate_tls_cipher_suites(
    owner_label: &str,
    cipher_suites: Option<&[TlsCipherSuiteConfig]>,
    versions: Option<&[TlsVersionConfig]>,
) -> Result<()> {
    let Some(cipher_suites) = cipher_suites else {
        return Ok(());
    };

    if cipher_suites.is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS cipher_suites must not be empty")));
    }

    let mut seen = HashSet::new();
    for suite in cipher_suites {
        if !seen.insert(*suite) {
            return Err(Error::Config(format!(
                "{owner_label} TLS cipher_suites must not contain duplicates"
            )));
        }
    }

    if let Some(versions) = versions
        && !cipher_suites.iter().any(|suite| {
            versions.iter().any(|version| cipher_suite_supports_version(*suite, *version))
        })
    {
        return Err(Error::Config(format!(
            "{owner_label} TLS cipher_suites do not support any configured TLS versions"
        )));
    }

    Ok(())
}

/// Validates the configured TLS key exchange groups.
pub(super) fn validate_tls_key_exchange_groups(
    owner_label: &str,
    key_exchange_groups: Option<&[TlsKeyExchangeGroupConfig]>,
) -> Result<()> {
    let Some(key_exchange_groups) = key_exchange_groups else {
        return Ok(());
    };

    if key_exchange_groups.is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS key_exchange_groups must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for group in key_exchange_groups {
        if !seen.insert(*group) {
            return Err(Error::Config(format!(
                "{owner_label} TLS key_exchange_groups must not contain duplicates"
            )));
        }
    }

    Ok(())
}

/// Validates the configured ALPN protocol list.
pub(super) fn validate_alpn_protocols(
    owner_label: &str,
    alpn_protocols: Option<&[String]>,
) -> Result<()> {
    let Some(alpn_protocols) = alpn_protocols else {
        return Ok(());
    };

    if alpn_protocols.is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS ALPN protocol list must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for protocol in alpn_protocols {
        let normalized = protocol.trim();
        if normalized.is_empty() {
            return Err(Error::Config(format!(
                "{owner_label} TLS ALPN protocol entries must not be empty"
            )));
        }

        if !seen.insert(normalized.to_ascii_lowercase()) {
            return Err(Error::Config(format!(
                "{owner_label} TLS ALPN protocol list must not contain duplicates"
            )));
        }
    }

    Ok(())
}

/// Validates a configured additional TLS certificate bundle.
fn validate_certificate_bundle(
    owner_label: &str,
    bundle: &ServerCertificateBundleConfig,
) -> Result<()> {
    if bundle.cert_path.trim().is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional certificate path must not be empty"
        )));
    }

    if bundle.key_path.trim().is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional private key path must not be empty"
        )));
    }

    if bundle.ocsp_staple_path.as_ref().is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional OCSP staple path must not be empty"
        )));
    }

    Ok(())
}

/// Returns whether a configured cipher suite is valid for a TLS version.
fn cipher_suite_supports_version(suite: TlsCipherSuiteConfig, version: TlsVersionConfig) -> bool {
    match suite {
        TlsCipherSuiteConfig::Tls13Aes256GcmSha384
        | TlsCipherSuiteConfig::Tls13Aes128GcmSha256
        | TlsCipherSuiteConfig::Tls13Chacha20Poly1305Sha256 => version == TlsVersionConfig::Tls13,
        TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes256GcmSha384
        | TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes128GcmSha256
        | TlsCipherSuiteConfig::TlsEcdheEcdsaWithChacha20Poly1305Sha256
        | TlsCipherSuiteConfig::TlsEcdheRsaWithAes256GcmSha384
        | TlsCipherSuiteConfig::TlsEcdheRsaWithAes128GcmSha256
        | TlsCipherSuiteConfig::TlsEcdheRsaWithChacha20Poly1305Sha256 => {
            version == TlsVersionConfig::Tls12
        }
    }
}
