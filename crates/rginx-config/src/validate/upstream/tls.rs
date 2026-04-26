use super::*;

pub(super) fn validate_tls_settings(upstream: &UpstreamConfig) -> Result<()> {
    let Some(tls) = &upstream.tls else {
        return Ok(());
    };

    if let UpstreamTlsModeConfig::CustomCa { ca_cert_path } = &tls.verify
        && ca_cert_path.trim().is_empty()
    {
        return Err(Error::Config(format!(
            "upstream `{}` custom CA path must not be empty",
            upstream.name
        )));
    }

    validate_tls_versions(&upstream.name, tls.versions.as_deref())?;

    if tls.verify_depth.is_some_and(|depth| depth == 0) {
        return Err(Error::Config(format!(
            "upstream `{}` verify_depth must be greater than 0",
            upstream.name
        )));
    }

    if tls.crl_path.as_ref().is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!(
            "upstream `{}` crl_path must not be empty",
            upstream.name
        )));
    }

    if matches!(tls.verify, UpstreamTlsModeConfig::Insecure)
        && (tls.verify_depth.is_some() || tls.crl_path.is_some())
    {
        return Err(Error::Config(format!(
            "upstream `{}` verify_depth and crl_path require certificate verification to remain enabled",
            upstream.name
        )));
    }

    match (&tls.client_cert_path, &tls.client_key_path) {
        (Some(cert_path), Some(key_path)) => {
            if cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` client_cert_path must not be empty",
                    upstream.name
                )));
            }

            if key_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` client_key_path must not be empty",
                    upstream.name
                )));
            }
        }
        (None, None) => {}
        _ => {
            return Err(Error::Config(format!(
                "upstream `{}` mTLS identity requires both client_cert_path and client_key_path",
                upstream.name
            )));
        }
    }

    Ok(())
}

fn validate_tls_versions(upstream_name: &str, versions: Option<&[TlsVersionConfig]>) -> Result<()> {
    let Some(versions) = versions else {
        return Ok(());
    };

    if versions.is_empty() {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` TLS versions must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for version in versions {
        if !seen.insert(version) {
            return Err(Error::Config(format!(
                "upstream `{upstream_name}` TLS versions must not contain duplicates"
            )));
        }
    }

    Ok(())
}
