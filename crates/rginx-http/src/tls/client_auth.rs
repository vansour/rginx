use std::sync::Arc;

use rustls::SignatureScheme;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};

#[derive(Debug)]
pub(super) struct DepthLimitedClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    verify_depth: Option<u32>,
}

impl DepthLimitedClientVerifier {
    pub(super) fn new(inner: Arc<dyn ClientCertVerifier>, verify_depth: Option<u32>) -> Self {
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
