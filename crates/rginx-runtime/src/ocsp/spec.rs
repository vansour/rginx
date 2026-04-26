use rginx_core::ConfigSnapshot;

pub(super) fn refresh_specs_for_config(
    config: &ConfigSnapshot,
) -> Vec<rginx_http::TlsOcspRefreshSpec> {
    rginx_http::tls_ocsp_refresh_specs_for_config(config)
}
