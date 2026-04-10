use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls_native_certs::load_native_certs;
use tokio::sync::watch;

use rginx_http::SharedState;

const OCSP_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const OCSP_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

type OcspClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub async fn run(state: SharedState, mut shutdown: watch::Receiver<bool>) {
    let client = match build_ocsp_client() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "dynamic OCSP client initialization failed");
            return;
        }
    };

    if let Err(error) = refresh_ocsp_staples(&state, &client).await {
        tracing::warn!(%error, "initial OCSP refresh failed");
    }

    let mut revisions = state.subscribe_updates();
    let mut interval = tokio::time::interval(OCSP_REFRESH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => break,
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            changed = revisions.changed() => {
                if changed.is_err() {
                    break;
                }
                if let Err(error) = refresh_ocsp_staples(&state, &client).await {
                    tracing::warn!(%error, "OCSP refresh after reload failed");
                }
            }
            _ = interval.tick() => {
                if let Err(error) = refresh_ocsp_staples(&state, &client).await {
                    tracing::warn!(%error, "periodic OCSP refresh failed");
                }
            }
        }
    }

    tracing::info!("dynamic OCSP refresher stopped");
}

async fn refresh_ocsp_staples(state: &SharedState, client: &OcspClient) -> Result<(), String> {
    let config = state.current_config().await;
    let mut tls_acceptors_changed = false;

    for ocsp in rginx_http::tls_ocsp_refresh_specs_for_config(config.as_ref()) {
        let Some(ocsp_staple_path) = ocsp.ocsp_staple_path.clone() else {
            continue;
        };
        if !ocsp.auto_refresh_enabled {
            continue;
        }

        let (request_body, request_nonce) =
            match rginx_http::build_ocsp_request_for_certificate_with_options(
                &ocsp.cert_path,
                ocsp.ocsp_nonce_mode,
            ) {
                Ok(request) => request,
                Err(error) => {
                    let (message, cache_cleared) = handle_ocsp_refresh_failure(
                        &ocsp.cert_path,
                        &ocsp_staple_path,
                        ocsp.ocsp_responder_policy,
                        error.to_string(),
                    )
                    .await;
                    if cache_cleared {
                        tls_acceptors_changed = true;
                    }
                    state.record_ocsp_refresh_failure(&ocsp.scope, message);
                    continue;
                }
            };

        match fetch_ocsp_response(client, &ocsp.responder_urls, request_body).await {
            Ok(response_body) => {
                if let Err(error) = rginx_http::validate_ocsp_response_for_certificate_with_options(
                    &ocsp.cert_path,
                    &response_body,
                    request_nonce.as_deref(),
                    ocsp.ocsp_nonce_mode,
                    ocsp.ocsp_responder_policy,
                ) {
                    let (message, cache_cleared) = handle_ocsp_refresh_failure(
                        &ocsp.cert_path,
                        &ocsp_staple_path,
                        ocsp.ocsp_responder_policy,
                        error.to_string(),
                    )
                    .await;
                    if cache_cleared {
                        tls_acceptors_changed = true;
                    }
                    state.record_ocsp_refresh_failure(&ocsp.scope, message);
                    continue;
                }
                match write_ocsp_cache_file(&ocsp_staple_path, &response_body).await {
                    Ok(changed) => {
                        if changed {
                            tls_acceptors_changed = true;
                        }
                        state.record_ocsp_refresh_success(&ocsp.scope);
                    }
                    Err(error) => {
                        let (message, cache_cleared) = handle_ocsp_refresh_failure(
                            &ocsp.cert_path,
                            &ocsp_staple_path,
                            ocsp.ocsp_responder_policy,
                            error,
                        )
                        .await;
                        if cache_cleared {
                            tls_acceptors_changed = true;
                        }
                        state.record_ocsp_refresh_failure(&ocsp.scope, message);
                    }
                }
            }
            Err(error) => {
                let (message, cache_cleared) = handle_ocsp_refresh_failure(
                    &ocsp.cert_path,
                    &ocsp_staple_path,
                    ocsp.ocsp_responder_policy,
                    error,
                )
                .await;
                if cache_cleared {
                    tls_acceptors_changed = true;
                }
                state.record_ocsp_refresh_failure(&ocsp.scope, message);
            }
        }
    }

    if tls_acceptors_changed {
        state.refresh_tls_acceptors_from_current_config().await.map_err(|error| {
            format!("failed to rebuild TLS acceptors after OCSP refresh: {error}")
        })?;
    }

    Ok(())
}

async fn fetch_ocsp_response(
    client: &OcspClient,
    responder_urls: &[String],
    request_body: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let mut errors = Vec::new();
    for responder_url in responder_urls {
        match fetch_ocsp_response_from_url(client, responder_url, request_body.clone()).await {
            Ok(response_body) => return Ok(response_body),
            Err(error) => errors.push(format!("{responder_url}: {error}")),
        }
    }

    Err(if errors.is_empty() {
        "no OCSP responder URLs were available".to_string()
    } else {
        errors.join("; ")
    })
}

async fn fetch_ocsp_response_from_url(
    client: &OcspClient,
    responder_url: &str,
    request_body: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let uri = responder_url
        .parse::<Uri>()
        .map_err(|error| format!("invalid OCSP responder URI: {error}"))?;
    let request = Request::post(uri)
        .header("content-type", "application/ocsp-request")
        .header("accept", "application/ocsp-response")
        .body(Full::new(Bytes::from(request_body)))
        .map_err(|error| format!("failed to build OCSP request: {error}"))?;

    let response = tokio::time::timeout(OCSP_FETCH_TIMEOUT, client.request(request))
        .await
        .map_err(|_| format!("timed out after {}s", OCSP_FETCH_TIMEOUT.as_secs()))?
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("responder returned HTTP {}", response.status()));
    }

    let mut body = response.into_body();
    let mut payload = Vec::new();
    while let Some(frame) = body
        .frame()
        .await
        .transpose()
        .map_err(|error| format!("failed to read OCSP response body: {error}"))?
    {
        let Some(chunk) = frame.data_ref() else {
            continue;
        };
        if payload.len().saturating_add(chunk.len()) > rginx_http::MAX_OCSP_RESPONSE_BYTES {
            return Err(format!(
                "OCSP response exceeded {} bytes",
                rginx_http::MAX_OCSP_RESPONSE_BYTES
            ));
        }
        payload.extend_from_slice(chunk);
    }
    if payload.is_empty() {
        return Err("responder returned an empty OCSP response body".to_string());
    }

    Ok(payload)
}

async fn write_ocsp_cache_file(path: &Path, body: &[u8]) -> Result<bool, String> {
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

async fn handle_ocsp_refresh_failure(
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

fn build_ocsp_client() -> Result<OcspClient, String> {
    let roots = load_native_root_store()?;
    let tls_config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .build();
    Ok(Client::builder(TokioExecutor::new()).build(connector))
}

fn load_native_root_store() -> Result<RootCertStore, String> {
    let result = load_native_certs();
    if !result.errors.is_empty() {
        tracing::warn!(errors = ?result.errors, "system root certificate loading reported errors");
    }
    let mut roots = RootCertStore::empty();
    let (added, ignored) = roots.add_parsable_certificates(result.certs);
    if ignored > 0 {
        tracing::warn!(ignored, "system root certificate loading ignored unparsable certificates");
    }
    if added == 0 {
        return Err(if result.errors.is_empty() {
            "no usable system root certificates were loaded for dynamic OCSP requests".to_string()
        } else {
            format!(
                "no usable system root certificates were loaded for dynamic OCSP requests ({} loader errors)",
                result.errors.len()
            )
        });
    }
    Ok(roots)
}
