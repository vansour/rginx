use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use super::*;
use rginx_core::{ClientIdentity, TlsVersion};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_native_certs::load_native_certs;

pub(super) fn build_tls_config(
    tls: &UpstreamTls,
    versions: Option<&[TlsVersion]>,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<&Path>,
    client_identity: Option<&ClientIdentity>,
    server_name: bool,
) -> Result<ClientConfig, Error> {
    let builder = build_client_config_builder(versions);
    let mut config = match tls {
        UpstreamTls::NativeRoots => {
            let roots = load_native_root_store()?;
            let verifier = build_server_cert_verifier(roots, server_verify_depth, server_crl_path)?;
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
        UpstreamTls::CustomCa { ca_cert_path } => {
            let roots = load_custom_ca_store(ca_cert_path)?;
            let verifier = build_server_cert_verifier(roots, server_verify_depth, server_crl_path)?;
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
        UpstreamTls::Insecure => {
            let verifier = Arc::new(InsecureServerCertVerifier::new());
            build_client_config_with_identity(
                builder.dangerous().with_custom_certificate_verifier(verifier),
                client_identity,
            )
        }
    }?;
    config.enable_sni = server_name;
    Ok(config)
}

fn build_server_cert_verifier(
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

fn build_client_config_builder(
    versions: Option<&[TlsVersion]>,
) -> rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier> {
    match versions {
        Some(versions) => ClientConfig::builder_with_protocol_versions(&rustls_versions(versions)),
        None => ClientConfig::builder(),
    }
}

fn build_client_config_with_identity(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>,
    client_identity: Option<&ClientIdentity>,
) -> Result<ClientConfig, Error> {
    match client_identity {
        Some(client_identity) => {
            let cert_chain = load_certificate_chain(&client_identity.cert_path)?;
            let key_der = load_private_key(&client_identity.key_path)?;
            builder.with_client_auth_cert(cert_chain, key_der).map_err(|error| {
                Error::Server(format!(
                    "failed to configure upstream mTLS identity from `{}` and `{}`: {error}",
                    client_identity.cert_path.display(),
                    client_identity.key_path.display()
                ))
            })
        }
        None => Ok(builder.with_no_client_auth()),
    }
}

pub(crate) fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let certs = load_certificate_chain(path)?;
    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

fn load_native_root_store() -> Result<RootCertStore, Error> {
    let result = load_native_certs();
    if result.certs.is_empty() && !result.errors.is_empty() {
        return Err(Error::Server(format!("failed to load native TLS roots: {:?}", result.errors)));
    }

    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(result.certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server("no valid native TLS roots were loaded".to_string()));
    }
    Ok(roots)
}

fn load_certificate_revocation_lists(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateRevocationListDer<'static>>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let crls =
        rustls_pemfile::crls(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!("failed to parse CRLs from `{}`: {error}", path.display()))
        })?;

    if !crls.is_empty() {
        return Ok(crls);
    }

    Ok(vec![rustls::pki_types::CertificateRevocationListDer::from(std::fs::read(path)?)])
}

fn load_certificate_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs =
        rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>().map_err(|error| {
            Error::Server(format!(
                "failed to parse certificates from `{}`: {error}",
                path.display()
            ))
        })?;

    if certs.is_empty() {
        let der = std::fs::read(path)?;
        return Ok(vec![CertificateDer::from(der)]);
    }

    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|error| {
            Error::Server(format!("failed to parse private key `{}`: {error}", path.display()))
        })?
        .ok_or_else(|| {
            Error::Server(format!(
                "private key file `{}` did not contain a supported PEM private key",
                path.display()
            ))
        })
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

#[derive(Debug)]
struct InsecureServerCertVerifier {
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
    fn new() -> Self {
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
