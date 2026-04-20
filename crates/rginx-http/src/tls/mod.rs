use rginx_core::{OcspNonceMode, OcspResponderPolicy, Result};

pub(crate) mod certificates;
pub(crate) mod ocsp;

mod acceptor;
mod client_auth;
mod provider;
mod session;
mod sni;
#[cfg(test)]
mod tests;

pub use acceptor::build_http3_server_config;
pub use acceptor::build_tls_acceptor;
#[cfg(test)]
pub(crate) use sni::best_matching_wildcard_certificates;

pub(crate) fn install_default_crypto_provider() {
    provider::install_default_crypto_provider();
}

pub fn build_ocsp_request_for_certificate(path: &std::path::Path) -> Result<Vec<u8>> {
    ocsp::build_ocsp_request_for_certificate(path)
}

pub fn build_ocsp_request_for_certificate_with_options(
    path: &std::path::Path,
    nonce_mode: OcspNonceMode,
) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    ocsp::build_ocsp_request_for_certificate_with_options(path, nonce_mode)
}

pub fn validate_ocsp_response_for_certificate(
    path: &std::path::Path,
    response_der: &[u8],
) -> Result<()> {
    ocsp::validate_ocsp_response_for_certificate(path, response_der)
}

pub fn validate_ocsp_response_for_certificate_with_options(
    path: &std::path::Path,
    response_der: &[u8],
    expected_nonce: Option<&[u8]>,
    nonce_mode: OcspNonceMode,
    responder_policy: OcspResponderPolicy,
) -> Result<()> {
    ocsp::validate_ocsp_response_for_certificate_with_options(
        path,
        response_der,
        expected_nonce,
        nonce_mode,
        responder_policy,
    )
}
