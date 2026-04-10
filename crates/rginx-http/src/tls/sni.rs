use std::collections::HashMap;
use std::sync::Arc;

use rginx_core::{ServerNameMatch, match_server_name};
use rustls::SignatureScheme;
use rustls::server::{ClientHello, ResolvesServerCert};

/// SNI 证书解析器，支持基于域名选择证书
#[derive(Debug)]
pub(super) struct SniCertificateResolver {
    default: Vec<Arc<rustls::sign::CertifiedKey>>,
    by_name: HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
}

impl SniCertificateResolver {
    pub(super) fn new(
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
            if let Some(certs) = self.by_name.get(&name_lower) {
                return select_compatible_certified_key(certs, client_hello.signature_schemes());
            }
            if let Some(certs) = best_matching_wildcard_certificates(&self.by_name, &name_lower) {
                return select_compatible_certified_key(certs, client_hello.signature_schemes());
            }
        }
        select_compatible_certified_key(&self.default, client_hello.signature_schemes())
    }
}

pub(super) fn register_server_name_certificates(
    by_name: &mut HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>>,
    server_names: &[String],
    certs: &[Arc<rustls::sign::CertifiedKey>],
) {
    for name in server_names {
        by_name.entry(name.to_lowercase()).or_default().extend(certs.iter().cloned());
    }
}

pub(crate) fn best_matching_wildcard_certificates<'a>(
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
