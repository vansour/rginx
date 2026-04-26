use serde::{Deserialize, Deserializer, de};

#[derive(Debug, Clone, Deserialize)]
pub struct ServerTlsConfig {
    pub cert_path: String,
    pub key_path: String,
    #[serde(default)]
    pub additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    #[serde(default)]
    pub versions: Option<Vec<TlsVersionConfig>>,
    #[serde(default)]
    pub cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
    #[serde(default)]
    pub key_exchange_groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
    #[serde(default)]
    pub alpn_protocols: Option<Vec<String>>,
    #[serde(default)]
    pub ocsp_staple_path: Option<String>,
    #[serde(default)]
    pub ocsp: Option<OcspConfig>,
    #[serde(default)]
    pub session_resumption: Option<bool>,
    #[serde(default)]
    pub session_tickets: Option<bool>,
    #[serde(default)]
    pub session_cache_size: Option<u64>,
    #[serde(default)]
    pub session_ticket_count: Option<u64>,
    #[serde(default)]
    pub client_auth: Option<ServerClientAuthConfig>,
}

#[derive(Debug, Clone)]
pub struct VirtualHostTlsConfig {
    pub cert_path: String,
    pub key_path: String,
    pub additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    pub ocsp_staple_path: Option<String>,
    pub ocsp: Option<OcspConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerCertificateBundleConfig {
    pub cert_path: String,
    pub key_path: String,
    #[serde(default)]
    pub ocsp_staple_path: Option<String>,
    #[serde(default)]
    pub ocsp: Option<OcspConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OcspConfig {
    #[serde(default)]
    pub nonce: Option<OcspNonceModeConfig>,
    #[serde(default)]
    pub responder_policy: Option<OcspResponderPolicyConfig>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum OcspNonceModeConfig {
    Disabled,
    Preferred,
    Required,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum OcspResponderPolicyConfig {
    IssuerOnly,
    IssuerOrDelegated,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsVersionConfig {
    Tls12,
    Tls13,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsCipherSuiteConfig {
    Tls13Aes256GcmSha384,
    Tls13Aes128GcmSha256,
    Tls13Chacha20Poly1305Sha256,
    TlsEcdheEcdsaWithAes256GcmSha384,
    TlsEcdheEcdsaWithAes128GcmSha256,
    TlsEcdheEcdsaWithChacha20Poly1305Sha256,
    TlsEcdheRsaWithAes256GcmSha384,
    TlsEcdheRsaWithAes128GcmSha256,
    TlsEcdheRsaWithChacha20Poly1305Sha256,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Hash)]
pub enum TlsKeyExchangeGroupConfig {
    X25519,
    Secp256r1,
    Secp384r1,
    X25519Mlkem768,
    Secp256r1Mlkem768,
    Mlkem768,
    Mlkem1024,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub enum ServerClientAuthModeConfig {
    Optional,
    Required,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerClientAuthConfig {
    pub mode: ServerClientAuthModeConfig,
    pub ca_cert_path: String,
    #[serde(default)]
    pub verify_depth: Option<u32>,
    #[serde(default)]
    pub crl_path: Option<String>,
}

impl<'de> Deserialize<'de> for VirtualHostTlsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Debug, Clone, Deserialize)]
        enum VirtualHostTlsConfigDe {
            VirtualHostTlsConfig {
                cert_path: String,
                key_path: String,
                #[serde(default)]
                additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
                #[serde(default)]
                ocsp_staple_path: Option<String>,
                #[serde(default)]
                ocsp: Option<OcspConfig>,
            },
            ServerTlsConfig {
                cert_path: String,
                key_path: String,
                #[serde(default)]
                additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
                #[serde(default)]
                versions: Option<Vec<TlsVersionConfig>>,
                #[serde(default)]
                cipher_suites: Option<Vec<TlsCipherSuiteConfig>>,
                #[serde(default)]
                key_exchange_groups: Option<Vec<TlsKeyExchangeGroupConfig>>,
                #[serde(default)]
                alpn_protocols: Option<Vec<String>>,
                #[serde(default)]
                ocsp_staple_path: Option<String>,
                #[serde(default)]
                ocsp: Option<OcspConfig>,
                #[serde(default)]
                session_resumption: Option<bool>,
                #[serde(default)]
                session_tickets: Option<bool>,
                #[serde(default)]
                session_cache_size: Option<u64>,
                #[serde(default)]
                session_ticket_count: Option<u64>,
                #[serde(default)]
                client_auth: Option<ServerClientAuthConfig>,
            },
        }

        match VirtualHostTlsConfigDe::deserialize(deserializer)? {
            VirtualHostTlsConfigDe::VirtualHostTlsConfig {
                cert_path,
                key_path,
                additional_certificates,
                ocsp_staple_path,
                ocsp,
            } => Ok(Self { cert_path, key_path, additional_certificates, ocsp_staple_path, ocsp }),
            VirtualHostTlsConfigDe::ServerTlsConfig {
                cert_path,
                key_path,
                additional_certificates,
                versions,
                cipher_suites,
                key_exchange_groups,
                alpn_protocols,
                ocsp_staple_path,
                ocsp,
                session_resumption,
                session_tickets,
                session_cache_size,
                session_ticket_count,
                client_auth,
            } => {
                if versions.is_some()
                    || cipher_suites.is_some()
                    || key_exchange_groups.is_some()
                    || alpn_protocols.is_some()
                    || session_resumption.is_some()
                    || session_tickets.is_some()
                    || session_cache_size.is_some()
                    || session_ticket_count.is_some()
                    || client_auth.is_some()
                {
                    return Err(de::Error::custom(
                        "vhost TLS policy fields are not supported in legacy `ServerTlsConfig(...)`; use `VirtualHostTlsConfig(...)` for certificate overrides and keep versions, cipher_suites, key_exchange_groups, ALPN, session settings, session cache settings, and client_auth on server.tls or listeners[].tls",
                    ));
                }

                Ok(Self { cert_path, key_path, additional_certificates, ocsp_staple_path, ocsp })
            }
        }
    }
}
