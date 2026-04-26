use bytes::Bytes;
use http_body_util::Full;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig;
use rustls::RootCertStore;
use rustls_native_certs::load_native_certs;

pub(super) type OcspClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub(super) fn build_ocsp_client() -> Result<OcspClient, String> {
    rginx_http::install_default_crypto_provider();
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
