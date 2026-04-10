use rginx_core::{Result, ServerTls, TlsCipherSuite, TlsKeyExchangeGroup, TlsVersion};
use rustls::SupportedCipherSuite;
use rustls::crypto::{CryptoProvider, SupportedKxGroup};

pub(super) fn default_crypto_provider() -> CryptoProvider {
    rustls::crypto::aws_lc_rs::default_provider()
}

pub(super) fn build_crypto_provider(tls: &ServerTls) -> Result<CryptoProvider> {
    let mut provider = default_crypto_provider();
    if let Some(cipher_suites) = tls.cipher_suites.as_deref() {
        provider.cipher_suites = cipher_suites
            .iter()
            .copied()
            .map(to_rustls_cipher_suite)
            .collect::<Result<Vec<_>>>()?;
    }
    if let Some(groups) = tls.key_exchange_groups.as_deref() {
        provider.kx_groups =
            groups.iter().copied().map(to_rustls_kx_group).collect::<Result<Vec<_>>>()?;
    }
    Ok(provider)
}

pub(super) fn rustls_versions(
    versions: &[TlsVersion],
) -> Vec<&'static rustls::SupportedProtocolVersion> {
    versions
        .iter()
        .map(|version| match version {
            TlsVersion::Tls12 => &rustls::version::TLS12,
            TlsVersion::Tls13 => &rustls::version::TLS13,
        })
        .collect()
}

fn to_rustls_cipher_suite(suite: TlsCipherSuite) -> Result<SupportedCipherSuite> {
    use rustls::crypto::aws_lc_rs::cipher_suite::*;

    Ok(match suite {
        TlsCipherSuite::Tls13Aes256GcmSha384 => TLS13_AES_256_GCM_SHA384,
        TlsCipherSuite::Tls13Aes128GcmSha256 => TLS13_AES_128_GCM_SHA256,
        TlsCipherSuite::Tls13Chacha20Poly1305Sha256 => TLS13_CHACHA20_POLY1305_SHA256,
        TlsCipherSuite::TlsEcdheEcdsaWithAes256GcmSha384 => TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        TlsCipherSuite::TlsEcdheEcdsaWithAes128GcmSha256 => TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        TlsCipherSuite::TlsEcdheEcdsaWithChacha20Poly1305Sha256 => {
            TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
        }
        TlsCipherSuite::TlsEcdheRsaWithAes256GcmSha384 => TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        TlsCipherSuite::TlsEcdheRsaWithAes128GcmSha256 => TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        TlsCipherSuite::TlsEcdheRsaWithChacha20Poly1305Sha256 => {
            TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
        }
    })
}

fn to_rustls_kx_group(group: TlsKeyExchangeGroup) -> Result<&'static dyn SupportedKxGroup> {
    use rustls::crypto::aws_lc_rs::kx_group::*;

    Ok(match group {
        TlsKeyExchangeGroup::X25519 => X25519,
        TlsKeyExchangeGroup::Secp256r1 => SECP256R1,
        TlsKeyExchangeGroup::Secp384r1 => SECP384R1,
        TlsKeyExchangeGroup::X25519Mlkem768 => X25519MLKEM768,
        TlsKeyExchangeGroup::Secp256r1Mlkem768 => SECP256R1MLKEM768,
        TlsKeyExchangeGroup::Mlkem768 => MLKEM768,
        TlsKeyExchangeGroup::Mlkem1024 => MLKEM1024,
    })
}
