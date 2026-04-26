use super::*;

pub(crate) fn https_client(
    cert_pem: &str,
) -> Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
> {
    let roots = root_store_from_pem(cert_pem).expect("root store should build");
    let client_config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("https client should support default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(client_config)
        .https_only()
        .enable_all_versions()
        .build();
    Client::builder(TokioExecutor::new()).build(connector)
}

pub(crate) fn root_store_from_pem(cert_pem: &str) -> Result<RootCertStore, String> {
    let cert = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse certificate PEM: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM did not contain a certificate".to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add(cert).map_err(|error| format!("failed to add root certificate: {error}"))?;
    Ok(roots)
}

pub(crate) fn load_certs_from_path(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    CertificateDer::pem_file_iter(path)
        .map_err(|error| format!("failed to open cert `{}`: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse cert `{}`: {error}", path.display()))
}

pub(crate) fn load_private_key_from_path(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|error| format!("failed to parse key `{}`: {error}", path.display()))
}
