use crate::state::SharedState;

const ACME_HTTP01_PREFIX: &str = "/.well-known/acme-challenge/";

pub(super) fn http01_response(state: &SharedState, request_path: &str) -> Option<String> {
    let token = request_path.strip_prefix(ACME_HTTP01_PREFIX)?;
    if token.is_empty() || token.contains('/') {
        return None;
    }

    state.acme_http01_response(token)
}
