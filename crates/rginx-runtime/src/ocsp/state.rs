use rginx_http::SharedState;

pub(super) fn record_refresh_success(state: &SharedState, scope: &str) {
    state.record_ocsp_refresh_success(scope);
}

pub(super) fn record_refresh_failure(state: &SharedState, scope: &str, message: String) {
    state.record_ocsp_refresh_failure(scope, message);
}

pub(super) async fn refresh_tls_acceptors_if_needed(
    state: &SharedState,
    tls_acceptors_changed: bool,
) -> Result<(), String> {
    if !tls_acceptors_changed {
        return Ok(());
    }

    state
        .refresh_tls_acceptors_from_current_config()
        .await
        .map_err(|error| format!("failed to rebuild TLS acceptors after OCSP refresh: {error}"))
}
