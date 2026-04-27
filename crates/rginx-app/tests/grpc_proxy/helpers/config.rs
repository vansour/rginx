use super::*;

pub(crate) fn tls_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    tls_proxy_config_with_request_timeout(listen_addr, upstream_addr, None)
}

pub(crate) fn tls_proxy_h2c_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            protocol: H2c,\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn tls_proxy_config_with_request_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    plain_proxy_config_with_request_timeout(listen_addr, upstream_addr, None)
}

pub(crate) fn plain_proxy_config_with_server_extra(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n{}    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        server_extra,
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_proxy_config_with_request_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_proxy_config_with_grpc_health_check(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n            health_check_grpc_service: Some(\"grpc.health.v1.Health\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(2),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_proxy_config_with_access_log(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"ACCESS reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\\\"$grpc_message\\\" route=$route\"),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_proxy_config_with_request_timeout_and_access_log(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"ACCESS reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\\\"$grpc_message\\\" route=$route\"),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}
pub(crate) fn tls_unmatched_grpc_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn plain_grpc_service_method_routing_config(
    listen_addr: SocketAddr,
    service_addr: SocketAddr,
    method_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"health-service\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n        UpstreamConfig(\n            name: \"health-check\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(\n                upstream: \"health-service\",\n            ),\n            grpc_service: Some(\"grpc.health.v1.Health\"),\n        ),\n        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(\n                upstream: \"health-check\",\n            ),\n            grpc_service: Some(\"grpc.health.v1.Health\"),\n            grpc_method: Some(\"Check\"),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", service_addr.port()),
        format!("https://127.0.0.1:{}", method_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}
