use std::path::Path;
use std::sync::Arc;

use quinn::crypto::rustls::QuicClientConfig;
use rginx_core::{ClientIdentity, Error, TlsVersion, UpstreamTls};
use rustls::ClientConfig;

use super::identity::{
    load_certificate_chain, load_custom_ca_store, load_native_root_store, load_private_key,
};
use super::verifier::{InsecureServerCertVerifier, build_server_cert_verifier};

pub(in crate::proxy::clients) fn build_tls_config(
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

pub(in crate::proxy::clients) fn build_http3_client_config(
    tls: &UpstreamTls,
    versions: Option<&[TlsVersion]>,
    server_verify_depth: Option<u32>,
    server_crl_path: Option<&Path>,
    client_identity: Option<&ClientIdentity>,
    server_name: bool,
) -> Result<quinn::ClientConfig, Error> {
    if let Some(versions) = versions
        && !versions.contains(&TlsVersion::Tls13)
    {
        return Err(Error::Config("upstream http3 requires TLS1.3 to remain enabled".to_string()));
    }

    let mut tls_config = build_tls_config(
        tls,
        Some(&[TlsVersion::Tls13]),
        server_verify_depth,
        server_crl_path,
        client_identity,
        server_name,
    )?;
    tls_config.alpn_protocols = vec![b"h3".to_vec()];

    let quic_config = QuicClientConfig::try_from(tls_config).map_err(|error| {
        Error::Server(format!("failed to build quic client config for upstream http3: {error}"))
    })?;

    Ok(quinn::ClientConfig::new(Arc::new(quic_config)))
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

fn rustls_versions(versions: &[TlsVersion]) -> Vec<&'static rustls::SupportedProtocolVersion> {
    versions
        .iter()
        .map(|version| match version {
            TlsVersion::Tls12 => &rustls::version::TLS12,
            TlsVersion::Tls13 => &rustls::version::TLS13,
        })
        .collect()
}
