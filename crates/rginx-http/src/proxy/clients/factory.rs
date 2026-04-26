use std::sync::Mutex;

use super::*;

pub(super) fn build_client_for_profile(
    profile: &UpstreamClientProfile,
) -> Result<ProxyClient, Error> {
    let resolver = Arc::new(UpstreamResolver::new(profile.dns.clone())?);
    if profile.protocol == UpstreamProtocol::Http3 {
        let client_config = tls::build_http3_client_config(
            &profile.tls,
            profile.tls_versions.as_deref(),
            profile.server_verify_depth,
            profile.server_crl_path.as_deref(),
            profile.client_identity.as_ref(),
            profile.server_name,
        )?;
        return Ok(ProxyClient::Http3(http3::Http3Client::new(
            client_config,
            profile.connect_timeout,
            resolver,
        )));
    }

    let tls_config = tls::build_tls_config(
        &profile.tls,
        profile.tls_versions.as_deref(),
        profile.server_verify_depth,
        profile.server_crl_path.as_deref(),
        profile.client_identity.as_ref(),
        profile.server_name,
    )?;
    let server_name_override = profile
        .server_name_override
        .as_ref()
        .map(|server_name_override| {
            ServerName::try_from(server_name_override.clone()).map_err(|error| {
                Error::Server(format!(
                    "invalid TLS server_name_override `{server_name_override}`: {error}"
                ))
            })
        })
        .transpose()?;

    Ok(ProxyClient::Http(Arc::new(HttpProxyClient {
        endpoint_clients: Arc::new(Mutex::new(EndpointClientCache::new(
            endpoint_client_cache_capacity(profile.pool_max_idle_per_host),
        ))),
        resolver,
        profile: profile.clone(),
        tls_config,
        server_name_override,
    })))
}
