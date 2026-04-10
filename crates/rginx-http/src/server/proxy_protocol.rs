use std::net::SocketAddr;

use tokio::io::AsyncReadExt;

const MAX_PROXY_PROTOCOL_HEADER_BYTES: usize = 108;

pub(super) async fn read_proxy_protocol_source_addr(
    stream: &mut tokio::net::TcpStream,
    remote_addr: SocketAddr,
    trust_remote_addr: bool,
) -> std::io::Result<Option<SocketAddr>> {
    let mut header = Vec::with_capacity(MAX_PROXY_PROTOCOL_HEADER_BYTES);
    loop {
        if header.len() >= MAX_PROXY_PROTOCOL_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proxy protocol header is too long",
            ));
        }

        let byte = stream.read_u8().await?;
        header.push(byte);
        if header.ends_with(b"\r\n") {
            break;
        }
    }

    let header = std::str::from_utf8(&header).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "proxy protocol header is not valid utf-8",
        )
    })?;
    parse_proxy_protocol_v1(header, remote_addr, trust_remote_addr)
}

pub(super) fn parse_proxy_protocol_v1(
    header: &str,
    remote_addr: SocketAddr,
    trust_remote_addr: bool,
) -> std::io::Result<Option<SocketAddr>> {
    let header = header.trim_end_matches("\r\n");
    if header == "PROXY UNKNOWN" {
        return Ok(None);
    }

    let mut parts = header.split_whitespace();
    let prefix = parts.next();
    let protocol = parts.next();
    let source_addr = parts.next();
    let _destination_addr = parts.next();
    let source_port = parts.next();
    let _destination_port = parts.next();
    let trailing = parts.next();

    if prefix != Some("PROXY") || trailing.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid proxy protocol header",
        ));
    }

    let source = match protocol {
        Some("TCP4") | Some("TCP6") => {
            let ip = source_addr
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing proxy protocol source address",
                    )
                })?
                .parse::<std::net::IpAddr>()
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid proxy protocol source address",
                    )
                })?;
            let port = source_port
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing proxy protocol source port",
                    )
                })?
                .parse::<u16>()
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid proxy protocol source port",
                    )
                })?;
            Some(SocketAddr::new(ip, port))
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported proxy protocol transport",
            ));
        }
    };

    if !trust_remote_addr {
        tracing::warn!(
            remote_addr = %remote_addr,
            "ignoring proxy protocol header because the transport peer is not trusted"
        );
        return Ok(None);
    }

    Ok(source)
}
