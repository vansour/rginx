use super::*;

pub(super) fn grpc_http3_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/grpc.health.v1.Health\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(super) fn grpc_http3_proxy_config_with_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    timeout_secs: u64,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            request_timeout_secs: Some({timeout_secs}),\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/grpc.health.v1.Health\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        timeout_secs = timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(super) fn grpc_http3_health_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            server_name_override: Some(\"localhost\"),\n            health_check_grpc_service: Some(\"grpc.health.v1.Health\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(1),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}
