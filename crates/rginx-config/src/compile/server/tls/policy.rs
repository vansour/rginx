use rginx_core::{Error, Result, TlsCipherSuite, TlsKeyExchangeGroup, TlsVersion};

use crate::model::{TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, TlsVersionConfig};

pub(super) fn compile_tls_versions(
    versions: Option<Vec<TlsVersionConfig>>,
) -> Option<Vec<TlsVersion>> {
    versions.map(|versions| {
        versions
            .into_iter()
            .map(|version| match version {
                TlsVersionConfig::Tls12 => TlsVersion::Tls12,
                TlsVersionConfig::Tls13 => TlsVersion::Tls13,
            })
            .collect()
    })
}

pub(super) fn compile_tls_cipher_suites(
    cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
) -> Option<Vec<TlsCipherSuite>> {
    cipher_suites.map(|cipher_suites| {
        cipher_suites
            .into_iter()
            .map(|suite| match suite {
                TlsCipherSuiteConfig::Tls13Aes256GcmSha384 => TlsCipherSuite::Tls13Aes256GcmSha384,
                TlsCipherSuiteConfig::Tls13Aes128GcmSha256 => TlsCipherSuite::Tls13Aes128GcmSha256,
                TlsCipherSuiteConfig::Tls13Chacha20Poly1305Sha256 => {
                    TlsCipherSuite::Tls13Chacha20Poly1305Sha256
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes256GcmSha384 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithAes256GcmSha384
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes128GcmSha256 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithAes128GcmSha256
                }
                TlsCipherSuiteConfig::TlsEcdheEcdsaWithChacha20Poly1305Sha256 => {
                    TlsCipherSuite::TlsEcdheEcdsaWithChacha20Poly1305Sha256
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithAes256GcmSha384 => {
                    TlsCipherSuite::TlsEcdheRsaWithAes256GcmSha384
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithAes128GcmSha256 => {
                    TlsCipherSuite::TlsEcdheRsaWithAes128GcmSha256
                }
                TlsCipherSuiteConfig::TlsEcdheRsaWithChacha20Poly1305Sha256 => {
                    TlsCipherSuite::TlsEcdheRsaWithChacha20Poly1305Sha256
                }
            })
            .collect()
    })
}

pub(super) fn compile_tls_key_exchange_groups(
    groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
) -> Option<Vec<TlsKeyExchangeGroup>> {
    groups.map(|groups| {
        groups
            .into_iter()
            .map(|group| match group {
                TlsKeyExchangeGroupConfig::X25519 => TlsKeyExchangeGroup::X25519,
                TlsKeyExchangeGroupConfig::Secp256r1 => TlsKeyExchangeGroup::Secp256r1,
                TlsKeyExchangeGroupConfig::Secp384r1 => TlsKeyExchangeGroup::Secp384r1,
                TlsKeyExchangeGroupConfig::X25519Mlkem768 => TlsKeyExchangeGroup::X25519Mlkem768,
                TlsKeyExchangeGroupConfig::Secp256r1Mlkem768 => {
                    TlsKeyExchangeGroup::Secp256r1Mlkem768
                }
                TlsKeyExchangeGroupConfig::Mlkem768 => TlsKeyExchangeGroup::Mlkem768,
                TlsKeyExchangeGroupConfig::Mlkem1024 => TlsKeyExchangeGroup::Mlkem1024,
            })
            .collect()
    })
}

pub(super) fn compile_alpn_protocols(alpn_protocols: Option<Vec<String>>) -> Option<Vec<String>> {
    alpn_protocols.map(|protocols| {
        protocols.into_iter().map(|protocol| protocol.trim().to_string()).collect()
    })
}

pub(super) fn compile_session_cache_size(session_cache_size: Option<u64>) -> Result<Option<usize>> {
    session_cache_size
        .map(|size| {
            usize::try_from(size).map_err(|_| {
                Error::Config(format!(
                    "server TLS session_cache_size `{size}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

pub(super) fn compile_session_ticket_count(
    session_ticket_count: Option<u64>,
) -> Result<Option<usize>> {
    session_ticket_count
        .map(|count| {
            usize::try_from(count).map_err(|_| {
                Error::Config(format!(
                    "server TLS session_ticket_count `{count}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}
