use std::path::{Path, PathBuf};

use rginx_core::{ClientIdentity, Error, Result, TlsVersion, UpstreamTls};

use crate::model::{TlsVersionConfig, UpstreamTlsConfig, UpstreamTlsModeConfig};

pub(super) struct CompiledUpstreamTls {
    pub(super) verify_mode: UpstreamTls,
    pub(super) tls_versions: Option<Vec<TlsVersion>>,
    pub(super) server_verify_depth: Option<u32>,
    pub(super) server_crl_path: Option<PathBuf>,
    pub(super) client_identity: Option<ClientIdentity>,
}

pub(super) fn compile_tls(
    upstream_name: &str,
    tls: Option<UpstreamTlsConfig>,
    base_dir: &Path,
) -> Result<CompiledUpstreamTls> {
    let tls = tls.unwrap_or(UpstreamTlsConfig {
        verify: UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        verify_depth: None,
        crl_path: None,
        client_cert_path: None,
        client_key_path: None,
    });

    let verify_mode = match tls.verify {
        UpstreamTlsModeConfig::NativeRoots => UpstreamTls::NativeRoots,
        UpstreamTlsModeConfig::Insecure => UpstreamTls::Insecure,
        UpstreamTlsModeConfig::CustomCa { ca_cert_path } => {
            let resolved = super::super::resolve_path(base_dir, ca_cert_path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` custom CA file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }

            UpstreamTls::CustomCa { ca_cert_path: resolved }
        }
    };

    let tls_versions = compile_tls_versions(&tls.versions);
    let server_verify_depth = tls.verify_depth;
    let server_crl_path = match tls.crl_path {
        Some(path) => {
            let resolved = super::super::resolve_path(base_dir, path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` CRL file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }
            Some(resolved)
        }
        None => None,
    };
    let client_identity = match (tls.client_cert_path, tls.client_key_path) {
        (None, None) => None,
        (Some(cert_path), Some(key_path)) => {
            let cert_path = super::super::resolve_path(base_dir, cert_path);
            if !cert_path.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` client certificate file `{}` does not exist or is not a file",
                    cert_path.display()
                )));
            }

            let key_path = super::super::resolve_path(base_dir, key_path);
            if !key_path.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` client private key file `{}` does not exist or is not a file",
                    key_path.display()
                )));
            }

            Some(ClientIdentity { cert_path, key_path })
        }
        _ => {
            return Err(Error::Config(format!(
                "upstream `{upstream_name}` mTLS identity requires both client_cert_path and client_key_path"
            )));
        }
    };

    Ok(CompiledUpstreamTls {
        verify_mode,
        tls_versions,
        server_verify_depth,
        server_crl_path,
        client_identity,
    })
}

fn compile_tls_versions(versions: &Option<Vec<TlsVersionConfig>>) -> Option<Vec<TlsVersion>> {
    versions.as_ref().map(|versions| {
        versions
            .iter()
            .map(|version| match version {
                TlsVersionConfig::Tls12 => TlsVersion::Tls12,
                TlsVersionConfig::Tls13 => TlsVersion::Tls13,
            })
            .collect()
    })
}
