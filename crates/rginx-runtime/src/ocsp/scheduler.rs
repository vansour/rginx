use rginx_http::SharedState;
use tokio::sync::watch;

use super::state::{
    record_refresh_failure, record_refresh_success, refresh_tls_acceptors_if_needed,
};
use super::{
    OCSP_REFRESH_INTERVAL, OcspClient, build_ocsp_client, fetch_ocsp_response,
    handle_ocsp_refresh_failure, spec, write_ocsp_cache_file,
};

pub(super) async fn run(state: SharedState, mut shutdown: watch::Receiver<bool>) {
    let client = match build_ocsp_client() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "dynamic OCSP client initialization failed");
            return;
        }
    };

    if let Err(error) = refresh_ocsp_staples_and_reload(&state, &client).await {
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
                if let Err(error) = refresh_ocsp_staples_and_reload(&state, &client).await {
                    tracing::warn!(%error, "OCSP refresh after reload failed");
                }
            }
            _ = interval.tick() => {
                if let Err(error) = refresh_ocsp_staples_and_reload(&state, &client).await {
                    tracing::warn!(%error, "periodic OCSP refresh failed");
                }
            }
        }
    }

    tracing::info!("dynamic OCSP refresher stopped");
}

pub(super) async fn refresh_now(state: &SharedState) -> Result<bool, String> {
    let client = build_ocsp_client()?;
    refresh_ocsp_staples(state, &client).await
}

async fn refresh_ocsp_staples_and_reload(
    state: &SharedState,
    client: &OcspClient,
) -> Result<(), String> {
    let tls_acceptors_changed = refresh_ocsp_staples(state, client).await?;
    refresh_tls_acceptors_if_needed(state, tls_acceptors_changed).await?;
    Ok(())
}

pub(super) async fn refresh_ocsp_staples(
    state: &SharedState,
    client: &OcspClient,
) -> Result<bool, String> {
    let config = state.current_config().await;
    let mut tls_acceptors_changed = false;

    for ocsp in spec::refresh_specs_for_config(config.as_ref()) {
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
                    tls_acceptors_changed |= cache_cleared;
                    record_refresh_failure(state, &ocsp.scope, message);
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
                    tls_acceptors_changed |= cache_cleared;
                    record_refresh_failure(state, &ocsp.scope, message);
                    continue;
                }
                match write_ocsp_cache_file(&ocsp_staple_path, &response_body).await {
                    Ok(changed) => {
                        tls_acceptors_changed |= changed;
                        record_refresh_success(state, &ocsp.scope);
                    }
                    Err(error) => {
                        let (message, cache_cleared) = handle_ocsp_refresh_failure(
                            &ocsp.cert_path,
                            &ocsp_staple_path,
                            ocsp.ocsp_responder_policy,
                            error,
                        )
                        .await;
                        tls_acceptors_changed |= cache_cleared;
                        record_refresh_failure(state, &ocsp.scope, message);
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
                tls_acceptors_changed |= cache_cleared;
                record_refresh_failure(state, &ocsp.scope, message);
            }
        }
    }

    Ok(tls_acceptors_changed)
}
