use super::*;

pub(super) fn validate_protocol_requirements(upstream: &UpstreamConfig) -> Result<()> {
    for peer in &upstream.peers {
        let uri = peer.url.parse::<http::Uri>().map_err(|error| {
            Error::Config(format!(
                "upstream `{}` peer url `{}` is not a valid URI: {error}",
                upstream.name, peer.url
            ))
        })?;

        match (&upstream.protocol, uri.scheme_str()) {
            (UpstreamProtocolConfig::Http2, Some("https"))
            | (UpstreamProtocolConfig::Http3, Some("https"))
            | (UpstreamProtocolConfig::H2c, Some("http")) => {}
            (UpstreamProtocolConfig::Http2, _) | (UpstreamProtocolConfig::Http3, _) => {
                let protocol = match &upstream.protocol {
                    UpstreamProtocolConfig::Http2 => "Http2",
                    UpstreamProtocolConfig::Http3 => "Http3",
                    _ => unreachable!("matched Http2 or Http3"),
                };
                return Err(Error::Config(format!(
                    "upstream `{}` protocol `{}` currently requires all peers to use `https://`; cleartext upstreams are not supported",
                    upstream.name, protocol
                )));
            }
            (UpstreamProtocolConfig::H2c, _) => {
                return Err(Error::Config(format!(
                    "upstream `{}` protocol `H2c` requires all peers to use `http://`",
                    upstream.name
                )));
            }
            _ => {}
        }
    }

    Ok(())
}
