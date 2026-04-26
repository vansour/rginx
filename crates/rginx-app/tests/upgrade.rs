use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[path = "upgrade/max_connections.rs"]
mod max_connections;
#[path = "upgrade/proxy_flow.rs"]
mod proxy_flow;
#[path = "upgrade/reload_flow.rs"]
mod reload_flow;

fn upgrade_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    upgrade_proxy_config_with_server_extra(listen_addr, upstream_addr, None)
}

fn upgrade_proxy_config_with_status_body(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    status_body: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/status\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({status_body:?}),\n            ),\n        ),\n        LocationConfig(\n            matcher: Prefix(\"/ws\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        status_body = status_body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn upgrade_proxy_config_with_server_extra(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: Option<&str>,
) -> String {
    let server_extra = server_extra.unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n{server_extra}    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/ws\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        server_extra = if server_extra.is_empty() {
            String::new()
        } else {
            format!("        {server_extra}\n")
        },
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn try_read_http_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream closed before the HTTP head was complete",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
    }
}
