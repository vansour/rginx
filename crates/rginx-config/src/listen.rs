use std::net::{Ipv4Addr, SocketAddr};

use rginx_core::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ParsedVhostListen {
    pub(crate) addr: SocketAddr,
    pub(crate) ssl: bool,
    pub(crate) http2: bool,
    pub(crate) http3: bool,
    pub(crate) proxy_protocol: bool,
}

pub(crate) fn parse_vhost_listen(owner_label: &str, raw: &str) -> Result<ParsedVhostListen> {
    let mut tokens = raw.split_whitespace();
    let Some(addr_token) = tokens.next() else {
        return Err(Error::Config(format!("{owner_label} listen must not be empty")));
    };

    let mut parsed = ParsedVhostListen {
        addr: parse_listen_addr(owner_label, addr_token)?,
        ssl: false,
        http2: false,
        http3: false,
        proxy_protocol: false,
    };

    for token in tokens {
        match token.to_ascii_lowercase().as_str() {
            "ssl" => parsed.ssl = true,
            "http2" => parsed.http2 = true,
            "http3" => {
                parsed.http3 = true;
                parsed.ssl = true;
            }
            "quic" => {
                parsed.http3 = true;
                parsed.ssl = true;
            }
            "proxy_protocol" => parsed.proxy_protocol = true,
            "default_server" | "reuseport" => {
                return Err(Error::Config(format!(
                    "{owner_label} listen option `{token}` is not supported yet"
                )));
            }
            _ => {
                return Err(Error::Config(format!(
                    "{owner_label} listen option `{token}` is not supported"
                )));
            }
        }
    }

    if parsed.http2 && !parsed.ssl {
        return Err(Error::Config(format!("{owner_label} listen http2 requires ssl")));
    }

    Ok(parsed)
}

fn parse_listen_addr(owner_label: &str, value: &str) -> Result<SocketAddr> {
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        let port = parse_port(owner_label, value)?;
        return Ok(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)));
    }

    let normalized = value
        .strip_prefix("*:")
        .map(|port| format!("0.0.0.0:{port}"))
        .unwrap_or_else(|| value.to_string());

    normalized.parse::<SocketAddr>().map_err(|error| {
        Error::Config(format!("{owner_label} listen `{value}` is invalid: {error}"))
    })
}

fn parse_port(owner_label: &str, value: &str) -> Result<u16> {
    value.parse::<u16>().map_err(|error| {
        Error::Config(format!("{owner_label} listen port `{value}` is invalid: {error}"))
    })
}
