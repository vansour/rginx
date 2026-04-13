use std::collections::HashMap;
use std::sync::Arc;

use rginx_core::{Error, Result, ServerClientAuthMode, ServerTls, TlsVersion, VirtualHost};
use rustls::ServerConfig;
use rustls::server::WebPkiClientVerifier;
use tokio_rustls::TlsAcceptor;

use super::certificates::{
    load_ca_cert_store, load_certificate_revocation_lists, load_certified_keys,
    load_vhost_certified_keys,
};
use super::client_auth::DepthLimitedClientVerifier;
use super::provider::{build_crypto_provider, default_crypto_provider, rustls_versions};
use super::session::apply_session_policy;
use super::sni::{SniCertificateResolver, register_server_name_certificates};

/// 构建支持 SNI 的 TLS acceptor
pub fn build_tls_acceptor(
    default_tls: Option<&ServerTls>,
    default_certificate: Option<&str>,
    tls_termination_enabled: bool,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<Option<TlsAcceptor>> {
    build_tls_server_config(
        default_tls,
        default_certificate,
        tls_termination_enabled,
        default_vhost,
        vhosts,
        default_tls
            .and_then(|tls| tls.alpn_protocols.clone())
            .unwrap_or_else(|| vec!["h2".to_string(), "http/1.1".to_string()]),
        false,
    )
    .map(|config| config.map(TlsAcceptor::from))
}

pub fn build_http3_server_config(
    default_tls: Option<&ServerTls>,
    default_certificate: Option<&str>,
    tls_termination_enabled: bool,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<Option<Arc<ServerConfig>>> {
    if default_tls.and_then(|tls| tls.client_auth.as_ref()).is_some() {
        return Err(Error::Config(
            "http3 currently does not support downstream client_auth on the same listener"
                .to_string(),
        ));
    }

    build_tls_server_config(
        default_tls,
        default_certificate,
        tls_termination_enabled,
        default_vhost,
        vhosts,
        vec!["h3".to_string()],
        true,
    )
}

fn build_tls_server_config(
    default_tls: Option<&ServerTls>,
    default_certificate: Option<&str>,
    tls_termination_enabled: bool,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
    alpn_protocols: Vec<String>,
    http3_only: bool,
) -> Result<Option<Arc<ServerConfig>>> {
    if !tls_termination_enabled {
        return Ok(None);
    }

    let mut all_certs: HashMap<String, Vec<Arc<rustls::sign::CertifiedKey>>> = HashMap::new();
    let mut default_certs = Vec::new();

    let mut listener_default_certs = Vec::new();
    if let Some(tls) = default_tls {
        let cert_keys = load_certified_keys(tls)?;
        listener_default_certs = cert_keys.clone();
        register_server_name_certificates(&mut all_certs, &default_vhost.server_names, &cert_keys);
    }

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
    let builder = build_server_config_builder(default_tls, http3_only)?;
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
    config.alpn_protocols = alpn_protocols.into_iter().map(String::into_bytes).collect();
    apply_session_policy(&mut config, default_tls)?;

    Ok(Some(Arc::new(config)))
}

fn build_server_config_builder(
    tls: Option<&ServerTls>,
    http3_only: bool,
) -> Result<rustls::ConfigBuilder<ServerConfig, rustls::WantsVerifier>> {
    let provider =
        tls.map(build_crypto_provider).transpose()?.unwrap_or_else(default_crypto_provider);
    let builder = ServerConfig::builder_with_provider(Arc::new(provider));
    if http3_only {
        if let Some(versions) = tls.and_then(|tls| tls.versions.as_deref())
            && !versions.contains(&TlsVersion::Tls13)
        {
            return Err(Error::Config(
                "http3 requires TLS1.3 to remain enabled on the same listener".to_string(),
            ));
        }

        return builder.with_protocol_versions(&[&rustls::version::TLS13]).map_err(|error| {
            Error::Server(format!(
                "failed to configure server TLS protocol versions for http3: {error}"
            ))
        });
    }

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
