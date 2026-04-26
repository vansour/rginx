use std::path::{Path, PathBuf};

use rginx_core::{
    Error, OcspConfig, OcspNonceMode, OcspResponderPolicy, Result, ServerCertificateBundle,
    ServerClientAuthMode, ServerClientAuthPolicy,
};

use crate::model::{
    OcspConfig as RawOcspConfig, OcspNonceModeConfig, OcspResponderPolicyConfig,
    ServerCertificateBundleConfig, ServerClientAuthConfig, ServerClientAuthModeConfig,
};

pub(super) struct CompiledCertificateMaterial {
    pub(super) cert_path: PathBuf,
    pub(super) key_path: PathBuf,
    pub(super) additional_certificates: Vec<ServerCertificateBundle>,
    pub(super) ocsp_staple_path: Option<PathBuf>,
    pub(super) ocsp: OcspConfig,
}

pub(super) fn compile_certificate_material(
    base_dir: &Path,
    cert_path: String,
    key_path: String,
    additional_certificates: Option<Vec<ServerCertificateBundleConfig>>,
    ocsp_staple_path: Option<String>,
    ocsp: Option<RawOcspConfig>,
    label: &str,
) -> Result<CompiledCertificateMaterial> {
    let cert_path = super::super::super::resolve_path(base_dir, cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "{label} certificate file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = super::super::super::resolve_path(base_dir, key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "{label} private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    let ocsp_staple_path = compile_ocsp_staple_path(base_dir, ocsp_staple_path, label)?;
    let ocsp = compile_ocsp_config(ocsp);
    let additional_certificates = additional_certificates
        .unwrap_or_default()
        .into_iter()
        .map(|bundle| {
            compile_certificate_bundle(base_dir, bundle, &format!("{label} additional certificate"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(CompiledCertificateMaterial {
        cert_path,
        key_path,
        additional_certificates,
        ocsp_staple_path,
        ocsp,
    })
}

pub(super) fn compile_client_auth_policy(
    base_dir: &Path,
    client_auth: ServerClientAuthConfig,
) -> Result<ServerClientAuthPolicy> {
    let ca_cert_path = super::super::super::resolve_path(base_dir, client_auth.ca_cert_path);
    if !ca_cert_path.is_file() {
        return Err(Error::Config(format!(
            "server TLS client auth CA file `{}` does not exist or is not a file",
            ca_cert_path.display()
        )));
    }

    let crl_path = match client_auth.crl_path {
        Some(path) => {
            let resolved = super::super::super::resolve_path(base_dir, path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "server TLS client auth CRL file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }
            Some(resolved)
        }
        None => None,
    };

    Ok(ServerClientAuthPolicy {
        mode: match client_auth.mode {
            ServerClientAuthModeConfig::Optional => ServerClientAuthMode::Optional,
            ServerClientAuthModeConfig::Required => ServerClientAuthMode::Required,
        },
        ca_cert_path,
        verify_depth: client_auth.verify_depth,
        crl_path,
    })
}

fn compile_certificate_bundle(
    base_dir: &Path,
    bundle: ServerCertificateBundleConfig,
    label: &str,
) -> Result<ServerCertificateBundle> {
    let cert_path = super::super::super::resolve_path(base_dir, bundle.cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "{label} file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = super::super::super::resolve_path(base_dir, bundle.key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "{label} private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    let ocsp_staple_path = compile_ocsp_staple_path(base_dir, bundle.ocsp_staple_path, label)?;
    let ocsp = compile_ocsp_config(bundle.ocsp);

    Ok(ServerCertificateBundle { cert_path, key_path, ocsp_staple_path, ocsp })
}

fn compile_ocsp_staple_path(
    base_dir: &Path,
    path: Option<String>,
    label: &str,
) -> Result<Option<PathBuf>> {
    match path {
        Some(path) => {
            let resolved = super::super::super::resolve_path(base_dir, path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "{label} OCSP staple file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }
            Ok(Some(resolved))
        }
        None => Ok(None),
    }
}

fn compile_ocsp_config(ocsp: Option<RawOcspConfig>) -> OcspConfig {
    let Some(ocsp) = ocsp else {
        return OcspConfig::default();
    };

    OcspConfig {
        nonce: ocsp
            .nonce
            .map(|value| match value {
                OcspNonceModeConfig::Disabled => OcspNonceMode::Disabled,
                OcspNonceModeConfig::Preferred => OcspNonceMode::Preferred,
                OcspNonceModeConfig::Required => OcspNonceMode::Required,
            })
            .unwrap_or_default(),
        responder_policy: ocsp
            .responder_policy
            .map(|value| match value {
                OcspResponderPolicyConfig::IssuerOnly => OcspResponderPolicy::IssuerOnly,
                OcspResponderPolicyConfig::IssuerOrDelegated => {
                    OcspResponderPolicy::IssuerOrDelegated
                }
            })
            .unwrap_or_default(),
    }
}
