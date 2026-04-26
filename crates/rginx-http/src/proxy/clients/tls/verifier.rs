use std::path::Path;
use std::sync::Arc;

use crate::tls::certificates::load_certificate_revocation_lists;
use rginx_core::Error;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

pub(super) fn build_server_cert_verifier(
    roots: RootCertStore,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<&Path>,
) -> Result<Arc<dyn ServerCertVerifier>, Error> {
    let builder = if let Some(crl_path) = server_crl_path {
        WebPkiServerVerifier::builder(roots.into())
            .with_crls(load_certificate_revocation_lists(crl_path)?)
    } else {
        WebPkiServerVerifier::builder(roots.into())
    };
    let verifier = builder.build().map_err(|error| {
        Error::Server(format!("failed to build upstream certificate verifier: {error}"))
    })?;
    Ok(Arc::new(DepthLimitedServerCertVerifier::new(verifier, server_verify_depth)))
}

#[derive(Debug)]
pub(super) struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

#[derive(Debug)]
struct DepthLimitedServerCertVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    verify_depth: Option<u32>,
}

impl DepthLimitedServerCertVerifier {
    fn new(inner: Arc<dyn ServerCertVerifier>, verify_depth: Option<u32>) -> Self {
        Self { inner, verify_depth }
    }
}

impl ServerCertVerifier for DepthLimitedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(max_depth) = self.verify_depth {
            let presented_chain_depth = 1usize.saturating_add(intermediates.len());
            if presented_chain_depth > max_depth as usize {
                return Err(rustls::Error::General(format!(
                    "upstream certificate chain exceeds configured verify_depth `{max_depth}`: got {presented_chain_depth} certificate(s)"
                )));
            }
        }

        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

impl InsecureServerCertVerifier {
    pub(super) fn new() -> Self {
        let supported_schemes = rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes();
        Self { supported_schemes }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}
