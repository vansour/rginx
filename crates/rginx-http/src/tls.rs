use std::collections::HashMap;
use std::sync::Arc;

use rginx_core::{
    Error, Result, ServerClientAuthMode, ServerNameMatch, ServerTls, TlsCipherSuite,
    TlsKeyExchangeGroup, TlsVersion, VirtualHost, match_server_name,
};
use rustls::SignatureScheme;
use rustls::crypto::CryptoProvider;
use rustls::crypto::SupportedKxGroup;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::ClientHello;
use rustls::server::NoServerSessionStorage;
use rustls::server::ProducesTickets;
use rustls::server::ResolvesServerCert;
use rustls::server::ServerSessionMemoryCache;
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ServerConfig, SupportedCipherSuite};
use tokio_rustls::TlsAcceptor;

pub(crate) mod certificates;

use self::certificates::{
    load_ca_cert_store, load_certificate_revocation_lists, load_certified_keys,
    load_vhost_certified_keys,
};

pub fn build_ocsp_request_for_certificate(path: &std::path::Path) -> Result<Vec<u8>> {
    certificates::build_ocsp_request_for_certificate(path)
}

/// SNI 证书解析器，支持基于域名选择证书
#[derive(Debug)]
pub struct SniCertificateResolver {
    default: Vec<Arc<rustls::sign::CertifiedKey>>,
    by_name: HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
}

impl SniCertificateResolver {
    pub fn new(
        default: Vec<Arc<rustls::sign::CertifiedKey>>,
        by_name: HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
    ) -> Self {
        Self { default, by_name }
    }
}

impl ResolvesServerCert for SniCertificateResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<rustls::sign::CertifiedKey>> {
        if let Some(name) = client_hello.server_name() {
            let name_lower = name.to_lowercase();
            // 先尝试精确匹配
            if let Some(certs) = self.by_name.get(&name_lower) {
                return select_compatible_certified_key(certs, client_hello.signature_schemes());
            }
            // 尝试选择最具体的通配符匹配
            if let Some(certs) = best_matching_wildcard_certificates(&self.by_name, &name_lower) {
                return select_compatible_certified_key(certs, client_hello.signature_schemes());
            }
        }
        select_compatible_certified_key(&self.default, client_hello.signature_schemes())
    }
}

/// 构建支持 SNI 的 TLS acceptor
pub fn build_tls_acceptor(
    default_tls: Option<&ServerTls>,
    default_certificate: Option<&str>,
    tls_termination_enabled: bool,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<Option<TlsAcceptor>> {
    if !tls_termination_enabled {
        return Ok(None);
    }

    // 收集所有 vhost 的证书
    let mut all_certs: HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>> = HashMap::new();
    let mut default_certs = Vec::new();

    // 处理 listener 默认 TLS
    let mut listener_default_certs = Vec::new();
    if let Some(tls) = default_tls {
        let cert_keys = load_certified_keys(tls)?;
        listener_default_certs = cert_keys.clone();
        register_server_name_certificates(&mut all_certs, &default_vhost.server_names, &cert_keys);
    }

    // 处理 default_vhost 的显式证书覆盖
    if let Some(tls) = &default_vhost.tls {
        let cert_keys = load_vhost_certified_keys(tls)?;
        register_server_name_certificates(&mut all_certs, &default_vhost.server_names, &cert_keys);
    }

    for vhost in vhosts {
        if let Some(tls) = &vhost.tls {
            let cert_keys = load_vhost_certified_keys(tls)?;
            register_server_name_certificates(&mut all_certs, &vhost.server_names, &cert_keys);
        }
    }

    if let Some(default_certificate) =
        default_certificate.map(str::trim).filter(|name| !name.is_empty())
    {
        let key = default_certificate.to_lowercase();
        let certs = all_certs.get(&key).cloned().ok_or_else(|| {
            Error::Config(format!(
                "default_certificate `{default_certificate}` does not match any TLS-enabled server_name"
            ))
        })?;
        default_certs = certs;
    }

    if default_certs.is_empty() {
        default_certs = listener_default_certs;
    }

    if default_certs.is_empty() && all_certs.len() == 1 {
        default_certs = all_certs.values().next().cloned().unwrap_or_default();
    }

    if default_certs.is_empty() && all_certs.is_empty() {
        return Ok(None);
    }

    let resolver = Arc::new(SniCertificateResolver::new(default_certs, all_certs));
    let builder = build_server_config_builder(default_tls)?;
    let mut config = if let Some(client_auth) = default_tls.and_then(|tls| tls.client_auth.as_ref())
    {
        let roots = load_ca_cert_store(&client_auth.ca_cert_path)?;
        let verifier_builder = if let Some(crl_path) = &client_auth.crl_path {
            WebPkiClientVerifier::builder(roots.into())
                .with_crls(load_certificate_revocation_lists(crl_path)?)
        } else {
            WebPkiClientVerifier::builder(roots.into())
        };
        let verifier = match client_auth.mode {
            ServerClientAuthMode::Optional => {
                verifier_builder.allow_unauthenticated().build().map_err(|error| {
                    Error::Server(format!(
                        "failed to build optional client verifier from `{}`: {error}",
                        client_auth.ca_cert_path.display()
                    ))
                })?
            }
            ServerClientAuthMode::Required => verifier_builder.build().map_err(|error| {
                Error::Server(format!(
                    "failed to build client verifier from `{}`: {error}",
                    client_auth.ca_cert_path.display()
                ))
            })?,
        };
        let verifier =
            Arc::new(DepthLimitedClientVerifier::new(verifier, client_auth.verify_depth));
        builder.with_client_cert_verifier(verifier).with_cert_resolver(resolver)
    } else {
        builder.with_no_client_auth().with_cert_resolver(resolver)
    };
    config.alpn_protocols = default_tls
        .and_then(|tls| tls.alpn_protocols.clone())
        .unwrap_or_else(|| vec!["h2".to_string(), "http/1.1".to_string()])
        .into_iter()
        .map(String::into_bytes)
        .collect();
    apply_session_policy(&mut config, default_tls)?;

    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

fn build_server_config_builder(
    tls: Option<&ServerTls>,
) -> Result<rustls::ConfigBuilder<ServerConfig, rustls::WantsVerifier>> {
    let provider =
        tls.map(build_crypto_provider).transpose()?.unwrap_or_else(default_crypto_provider);
    let builder = ServerConfig::builder_with_provider(Arc::new(provider));
    match tls.and_then(|tls| tls.versions.as_deref()) {
        Some(versions) => {
            builder.with_protocol_versions(&rustls_versions(versions)).map_err(|error| {
                Error::Server(format!("failed to configure server TLS protocol versions: {error}"))
            })
        }
        None => builder.with_safe_default_protocol_versions().map_err(|error| {
            Error::Server(format!("failed to configure server TLS protocol versions: {error}"))
        }),
    }
}

fn default_crypto_provider() -> CryptoProvider {
    rustls::crypto::aws_lc_rs::default_provider()
}

fn build_crypto_provider(tls: &ServerTls) -> Result<CryptoProvider> {
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

fn apply_session_policy(config: &mut ServerConfig, tls: Option<&ServerTls>) -> Result<()> {
    let Some(tls) = tls else {
        return Ok(());
    };

    if matches!(tls.session_resumption, Some(false)) {
        config.session_storage = Arc::new(NoServerSessionStorage {});
        config.ticketer = Arc::new(DisabledTicketProducer {});
        config.send_tls13_tickets = 0;
        return Ok(());
    }

    if let Some(session_cache_size) = tls.session_cache_size {
        config.session_storage = if session_cache_size == 0 {
            Arc::new(NoServerSessionStorage {})
        } else {
            ServerSessionMemoryCache::new(session_cache_size)
        };
    }

    if matches!(tls.session_tickets, Some(false)) {
        config.ticketer = Arc::new(DisabledTicketProducer {});
        config.send_tls13_tickets = 0;
    } else if matches!(tls.session_tickets, Some(true)) || tls.session_ticket_count.is_some() {
        config.ticketer = rustls::crypto::aws_lc_rs::Ticketer::new().map_err(|error| {
            Error::Server(format!("failed to enable server TLS session tickets: {error}"))
        })?;
        config.send_tls13_tickets = tls.session_ticket_count.unwrap_or(2);
    }

    Ok(())
}

#[derive(Debug)]
struct DisabledTicketProducer {}

impl ProducesTickets for DisabledTicketProducer {
    fn enabled(&self) -> bool {
        false
    }

    fn lifetime(&self) -> u32 {
        0
    }

    fn encrypt(&self, _plain: &[u8]) -> Option<Vec<u8>> {
        None
    }

    fn decrypt(&self, _cipher: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Debug)]
struct DepthLimitedClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    verify_depth: Option<u32>,
}

impl DepthLimitedClientVerifier {
    fn new(inner: Arc<dyn ClientCertVerifier>, verify_depth: Option<u32>) -> Self {
        Self { inner, verify_depth }
    }
}

impl ClientCertVerifier for DepthLimitedClientVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        if let Some(max_depth) = self.verify_depth {
            let presented_chain_depth = 1usize.saturating_add(intermediates.len());
            if presented_chain_depth > max_depth as usize {
                return Err(rustls::Error::General(format!(
                    "client certificate chain exceeds configured verify_depth `{max_depth}`: got {presented_chain_depth} certificate(s)"
                )));
            }
        }

        self.inner.verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn rustls_versions(versions: &[TlsVersion]) -> Vec<&'static rustls::SupportedProtocolVersion> {
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

fn register_server_name_certificates(
    by_name: &mut HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
    server_names: &[String],
    certs: &[Arc<rustls::sign::CertifiedKey>],
) {
    for name in server_names {
        by_name.entry(name.to_lowercase()).or_default().extend(certs.iter().cloned());
    }
}

fn best_matching_wildcard_certificates<'a>(
    by_name: &'a HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
    server_name: &str,
) -> Option<&'a Vec<Arc<rustls::sign::CertifiedKey>>> {
    by_name
        .iter()
        .filter_map(|(pattern, certs)| match match_server_name(pattern, server_name) {
            Some(ServerNameMatch::Wildcard { suffix_len }) => Some((suffix_len, pattern, certs)),
            _ => None,
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(left.1)))
        .map(|(_, _, certs)| certs)
}

fn select_compatible_certified_key(
    candidates: &[Arc<rustls::sign::CertifiedKey>],
    signature_schemes: &[SignatureScheme],
) -> Option<Arc<rustls::sign::CertifiedKey>> {
    candidates
        .iter()
        .find(|candidate| candidate.key.choose_scheme(signature_schemes).is_some())
        .cloned()
        .or_else(|| candidates.first().cloned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rginx_core::{ServerTls, TlsCipherSuite, TlsKeyExchangeGroup, VirtualHost, VirtualHostTls};
    use rustls::{CipherSuite, NamedGroup};

    use super::{best_matching_wildcard_certificates, build_tls_acceptor};

    const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
    const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

    #[test]
    fn build_tls_acceptor_returns_none_for_plain_http() {
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        };
        let vhosts: Vec<VirtualHost> = Vec::new();

        assert!(build_tls_acceptor(None, None, false, &default_vhost, &vhosts).unwrap().is_none());
    }

    #[test]
    fn build_tls_acceptor_loads_valid_pem_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

        let server_tls = rginx_core::ServerTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            versions: None,
            cipher_suites: None,
            key_exchange_groups: None,
            alpn_protocols: None,
            ocsp_staple_path: None,
            session_resumption: None,
            session_tickets: None,
            session_cache_size: None,
            session_ticket_count: None,
            client_auth: None,
        };
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: None,
        };
        let vhosts: Vec<VirtualHost> = Vec::new();

        let acceptor = build_tls_acceptor(Some(&server_tls), None, true, &default_vhost, &vhosts)
            .expect("TLS acceptor should load");
        assert!(acceptor.is_some());
        assert_eq!(
            acceptor
                .expect("TLS acceptor should exist")
                .config()
                .alpn_protocols
                .iter()
                .map(|protocol| protocol.as_slice())
                .collect::<Vec<_>>(),
            vec![b"h2".as_slice(), b"http/1.1".as_slice()]
        );

        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn build_tls_acceptor_respects_custom_alpn_protocols() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-alpn-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

        let tls = ServerTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            versions: None,
            cipher_suites: None,
            key_exchange_groups: None,
            alpn_protocols: Some(vec!["http/1.1".to_string()]),
            ocsp_staple_path: None,
            session_resumption: None,
            session_tickets: None,
            session_cache_size: None,
            session_ticket_count: None,
            client_auth: None,
        };
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: None,
        };

        let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
            .expect("TLS acceptor should load")
            .expect("TLS acceptor should exist");
        assert_eq!(
            acceptor
                .config()
                .alpn_protocols
                .iter()
                .map(|protocol| protocol.as_slice())
                .collect::<Vec<_>>(),
            vec![b"http/1.1".as_slice()]
        );

        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn build_tls_acceptor_rejects_unknown_default_certificate_name() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("rginx-server-tls-default-cert-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

        let vhosts = vec![VirtualHost {
            id: "servers[0]".to_string(),
            server_names: vec!["app.example.com".to_string()],
            routes: Vec::new(),
            tls: Some(VirtualHostTls {
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
                additional_certificates: Vec::new(),
                ocsp_staple_path: None,
            }),
        }];
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        };

        let error = match build_tls_acceptor(
            None,
            Some("missing.example.com"),
            true,
            &default_vhost,
            &vhosts,
        ) {
            Ok(_) => panic!("unknown default_certificate should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("default_certificate `missing.example.com`"));

        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn build_tls_acceptor_uses_single_vhost_cert_as_implicit_default() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir =
            std::env::temp_dir().join(format!("rginx-server-tls-single-default-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");

        let vhosts = vec![VirtualHost {
            id: "servers[0]".to_string(),
            server_names: vec!["app.example.com".to_string()],
            routes: Vec::new(),
            tls: Some(VirtualHostTls {
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
                additional_certificates: Vec::new(),
                ocsp_staple_path: None,
            }),
        }];
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        };

        let acceptor = build_tls_acceptor(None, None, true, &default_vhost, &vhosts)
            .expect("single vhost cert should become implicit default");
        assert!(acceptor.is_some());

        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }

    #[test]
    fn wildcard_sni_selection_prefers_more_specific_patterns() {
        let certs = vec![Arc::new(dummy_certified_key())];
        let by_name = HashMap::from([
            ("*.example.com".to_string(), certs.clone()),
            ("*.api.example.com".to_string(), certs.clone()),
        ]);

        let selected = best_matching_wildcard_certificates(&by_name, "edge.api.example.com")
            .expect("more specific wildcard should match");
        assert_eq!(selected.len(), 1);
        assert!(best_matching_wildcard_certificates(&by_name, "example.com").is_none());
    }

    #[test]
    fn build_tls_acceptor_applies_custom_cipher_suites_and_groups() {
        let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-policy");
        let tls = ServerTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            versions: Some(vec![rginx_core::TlsVersion::Tls13]),
            cipher_suites: Some(vec![TlsCipherSuite::Tls13Aes128GcmSha256]),
            key_exchange_groups: Some(vec![TlsKeyExchangeGroup::Secp256r1]),
            alpn_protocols: None,
            ocsp_staple_path: None,
            session_resumption: None,
            session_tickets: None,
            session_cache_size: None,
            session_ticket_count: None,
            client_auth: None,
        };
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: None,
        };

        let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
            .expect("TLS acceptor should load")
            .expect("TLS acceptor should exist");
        assert_eq!(
            acceptor.config().crypto_provider().cipher_suites[0].suite(),
            CipherSuite::TLS13_AES_128_GCM_SHA256
        );
        assert_eq!(acceptor.config().crypto_provider().kx_groups[0].name(), NamedGroup::secp256r1);

        remove_test_cert_pair(cert_path, key_path, temp_dir);
    }

    #[test]
    fn build_tls_acceptor_disables_session_resumption_when_requested() {
        let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-resumption");
        let tls = ServerTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            versions: None,
            cipher_suites: None,
            key_exchange_groups: None,
            alpn_protocols: None,
            ocsp_staple_path: None,
            session_resumption: Some(false),
            session_tickets: Some(false),
            session_cache_size: Some(0),
            session_ticket_count: None,
            client_auth: None,
        };
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: None,
        };

        let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
            .expect("TLS acceptor should load")
            .expect("TLS acceptor should exist");
        assert!(!acceptor.config().session_storage.can_cache());
        assert!(!acceptor.config().ticketer.enabled());
        assert_eq!(acceptor.config().send_tls13_tickets, 0);

        remove_test_cert_pair(cert_path, key_path, temp_dir);
    }

    #[test]
    fn build_tls_acceptor_enables_session_tickets_when_requested() {
        let (cert_path, key_path, temp_dir) = write_test_cert_pair("rginx-server-tls-tickets");
        let tls = ServerTls {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            additional_certificates: Vec::new(),
            versions: None,
            cipher_suites: None,
            key_exchange_groups: None,
            alpn_protocols: None,
            ocsp_staple_path: None,
            session_resumption: Some(true),
            session_tickets: Some(true),
            session_cache_size: Some(2),
            session_ticket_count: Some(4),
            client_auth: None,
        };
        let default_vhost = VirtualHost {
            id: "server".to_string(),
            server_names: vec!["localhost".to_string()],
            routes: Vec::new(),
            tls: None,
        };

        let acceptor = build_tls_acceptor(Some(&tls), None, true, &default_vhost, &[])
            .expect("TLS acceptor should load")
            .expect("TLS acceptor should exist");
        assert!(acceptor.config().session_storage.can_cache());
        assert!(acceptor.config().ticketer.enabled());
        assert_eq!(acceptor.config().send_tls13_tickets, 4);

        let storage = &acceptor.config().session_storage;
        assert!(storage.put(vec![0x01], vec![0x0a]));
        assert!(storage.put(vec![0x02], vec![0x0b]));
        assert!(storage.put(vec![0x03], vec![0x0c]));
        let count = storage.get(&[0x01]).iter().count()
            + storage.get(&[0x02]).iter().count()
            + storage.get(&[0x03]).iter().count();
        assert!(count < 3);

        remove_test_cert_pair(cert_path, key_path, temp_dir);
    }

    fn dummy_certified_key() -> rustls::sign::CertifiedKey {
        let cert_path = std::env::temp_dir().join("rginx-unused-test-cert.pem");
        let key_path = std::env::temp_dir().join("rginx-unused-test-key.pem");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("dummy cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("dummy key should be written");
        let certified = self::super::certificates::load_certified_key_bundle(
            &rginx_core::ServerCertificateBundle {
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
                ocsp_staple_path: None,
            },
        )
        .expect("dummy certified key should load");
        let _ = fs::remove_file(cert_path);
        let _ = fs::remove_file(key_path);
        Arc::unwrap_or_clone(certified)
    }

    fn write_test_cert_pair(
        prefix: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("{prefix}-{unique}"));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("test cert should be written");
        fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("test key should be written");
        (cert_path, key_path, temp_dir)
    }

    fn remove_test_cert_pair(
        cert_path: std::path::PathBuf,
        key_path: std::path::PathBuf,
        temp_dir: std::path::PathBuf,
    ) {
        fs::remove_file(cert_path).expect("test cert should be removed");
        fs::remove_file(key_path).expect("test key should be removed");
        fs::remove_dir(temp_dir).expect("temp dir should be removed");
    }
}
