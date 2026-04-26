use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn write_ocsp_cache_file(path: &Path, body: &[u8]) -> Result<bool, String> {
    if tokio::fs::read(path).await.ok().as_deref() == Some(body) {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            format!("failed to create OCSP cache directory `{}`: {error}", parent.display())
        })?;
    }

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = path.with_extension(format!("ocsp-{unique}.tmp"));
    tokio::fs::write(&temp_path, body).await.map_err(|error| {
        format!("failed to write OCSP cache file `{}`: {error}", temp_path.display())
    })?;
    tokio::fs::rename(&temp_path, path).await.map_err(|error| {
        format!("failed to replace OCSP cache file `{}`: {error}", path.display())
    })?;
    Ok(true)
}

pub(super) async fn handle_ocsp_refresh_failure(
    cert_path: &Path,
    cache_path: &Path,
    responder_policy: rginx_core::OcspResponderPolicy,
    error: String,
) -> (String, bool) {
    match clear_invalid_ocsp_cache_file(cert_path, cache_path, responder_policy).await {
        Ok(true) => (format!("{error}; cleared stale OCSP cache"), true),
        Ok(false) => (error, false),
        Err(clear_error) => {
            (format!("{error}; additionally failed to clear stale OCSP cache: {clear_error}"), true)
        }
    }
}

async fn clear_invalid_ocsp_cache_file(
    cert_path: &Path,
    cache_path: &Path,
    responder_policy: rginx_core::OcspResponderPolicy,
) -> Result<bool, String> {
    let metadata = match tokio::fs::metadata(cache_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "failed to stat OCSP cache file `{}`: {error}",
                cache_path.display()
            ));
        }
    };
    if metadata.len() == 0 {
        return Ok(false);
    }
    if metadata.len() > rginx_http::MAX_OCSP_RESPONSE_BYTES as u64 {
        return clear_ocsp_cache_file(cache_path).await;
    }

    let body = match tokio::fs::read(cache_path).await {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "failed to read OCSP cache file `{}`: {error}",
                cache_path.display()
            ));
        }
    };
    if body.is_empty() {
        return Ok(false);
    }

    if rginx_http::validate_ocsp_response_for_certificate_with_options(
        cert_path,
        &body,
        None,
        rginx_core::OcspNonceMode::Disabled,
        responder_policy,
    )
    .is_ok()
    {
        return Ok(false);
    }

    clear_ocsp_cache_file(cache_path).await
}

async fn clear_ocsp_cache_file(path: &Path) -> Result<bool, String> {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!("failed to stat OCSP cache file `{}`: {error}", path.display()));
        }
    };
    if metadata.len() == 0 {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            format!("failed to create OCSP cache directory `{}`: {error}", parent.display())
        })?;
    }

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp_path = path.with_extension(format!("ocsp-clear-{unique}.tmp"));
    tokio::fs::write(&temp_path, []).await.map_err(|error| {
        format!("failed to clear OCSP cache file `{}`: {error}", temp_path.display())
    })?;
    tokio::fs::rename(&temp_path, path).await.map_err(|error| {
        format!("failed to replace cleared OCSP cache file `{}`: {error}", path.display())
    })?;
    Ok(true)
}
