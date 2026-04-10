use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcspNonceMode {
    #[default]
    Disabled,
    Preferred,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OcspResponderPolicy {
    IssuerOnly,
    #[default]
    IssuerOrDelegated,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct OcspConfig {
    pub nonce: OcspNonceMode,
    pub responder_policy: OcspResponderPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerCertificateBundle {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub ocsp_staple_path: Option<PathBuf>,
    pub ocsp: OcspConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsCipherSuite {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsKeyExchangeGroup {
    X25519,
    Secp256r1,
    Secp384r1,
    X25519Mlkem768,
    Secp256r1Mlkem768,
    Mlkem768,
    Mlkem1024,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerClientAuthMode {
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerClientAuthPolicy {
    pub mode: ServerClientAuthMode,
    pub ca_cert_path: PathBuf,
    pub verify_depth: Option<u32>,
    pub crl_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientIdentity {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VirtualHostTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub additional_certificates: Vec<ServerCertificateBundle>,
    pub ocsp_staple_path: Option<PathBuf>,
    pub ocsp: OcspConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub additional_certificates: Vec<ServerCertificateBundle>,
    pub versions: Option<Vec<TlsVersion>>,
    pub cipher_suites: Option<Vec<TlsCipherSuite>>,
    pub key_exchange_groups: Option<Vec<TlsKeyExchangeGroup>>,
    pub alpn_protocols: Option<Vec<String>>,
    pub ocsp_staple_path: Option<PathBuf>,
    pub ocsp: OcspConfig,
    pub session_resumption: Option<bool>,
    pub session_tickets: Option<bool>,
    pub session_cache_size: Option<usize>,
    pub session_ticket_count: Option<usize>,
    pub client_auth: Option<ServerClientAuthPolicy>,
}
