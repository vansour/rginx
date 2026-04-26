use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{
    TlsVersionConfig, UpstreamConfig, UpstreamProtocolConfig, UpstreamTlsModeConfig,
};

mod basics;
mod dns;
mod health;
mod protocol;
mod tls;
mod tuning;

pub(super) fn validate_upstreams(upstreams: &[UpstreamConfig]) -> Result<HashSet<String>> {
    let mut upstream_names = HashSet::new();

    for upstream in upstreams {
        basics::validate_upstream_name_and_peers(upstream, &mut upstream_names)?;
        tls::validate_tls_settings(upstream)?;
        basics::validate_server_name_override(upstream)?;
        protocol::validate_protocol_requirements(upstream)?;
        dns::validate_dns_settings(upstream)?;
        tuning::validate_timeout_and_tuning(upstream)?;
        health::validate_active_health_settings(upstream)?;
    }

    Ok(upstream_names)
}
