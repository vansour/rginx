use super::*;

pub(super) fn validate_protocol_requirements(upstream: &UpstreamConfig) -> Result<()> {
    if !matches!(upstream.protocol, UpstreamProtocolConfig::Http2 | UpstreamProtocolConfig::Http3) {
        return Ok(());
    }

    for peer in &upstream.peers {
        let uri = peer.url.parse::<http::Uri>().map_err(|error| {
            Error::Config(format!(
                "upstream `{}` peer url `{}` is not a valid URI: {error}",
                upstream.name, peer.url
            ))
        })?;

        if uri.scheme_str() != Some("https") {
            return Err(Error::Config(format!(
                "upstream `{}` protocol `{}` currently requires all peers to use `https://`; cleartext upstreams are not supported",
                upstream.name,
                match upstream.protocol {
                    UpstreamProtocolConfig::Http2 => "Http2",
                    UpstreamProtocolConfig::Http3 => "Http3",
                    _ => unreachable!("guarded by matches!"),
                }
            )));
        }
    }

    Ok(())
}
