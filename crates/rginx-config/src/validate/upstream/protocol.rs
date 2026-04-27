use super::*;

pub(super) fn validate_protocol_requirements(upstream: &UpstreamConfig) -> Result<()> {
    for peer in &upstream.peers {
        let uri = peer.url.parse::<http::Uri>().map_err(|error| {
            Error::Config(format!(
                "upstream `{}` peer url `{}` is not a valid URI: {error}",
                upstream.name, peer.url
            ))
        })?;

        match upstream.protocol {
            UpstreamProtocolConfig::Http2 | UpstreamProtocolConfig::Http3
                if uri.scheme_str() != Some("https") =>
            {
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
            UpstreamProtocolConfig::H2c if uri.scheme_str() != Some("http") => {
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
