use super::*;

pub(crate) fn http3_return_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"fallback\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"localhost\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/v3\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"http3 return\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_policy_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/allow\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"allowed\\n\"),\n            ),\n            allow_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n        LocationConfig(\n            matcher: Exact(\"/deny\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"denied\\n\"),\n            ),\n            deny_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n        LocationConfig(\n            matcher: Exact(\"/limited\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"limited\\n\"),\n            ),\n            requests_per_sec: Some(1),\n            burst: Some(0),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_compression_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        access_log_format: Some(\"H3 reqid=$request_id version=$http_version status=$status\"),\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/gzip\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        "http3 gzip body\n".repeat(32),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                preserve_host: Some(false),\n                strip_prefix: Some(\"/api\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_streaming_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                preserve_host: Some(false),\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_retry_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    host_key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            active_connection_id_limit: Some(5),\n            retry: Some(true),\n            host_key_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"fallback\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"localhost\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/v3\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"http3 return\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        host_key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_early_data_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            early_data: Some(true),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/safe\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"early data safe\\n\"),\n            ),\n            allow_early_data: Some(true),\n        ),\n        LocationConfig(\n            matcher: Exact(\"/unsafe\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"early data unsafe\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(crate) fn http3_client_auth_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    ca_path: &std::path::Path,
    mode: &str,
) -> String {
    let body = if mode == "Required" { "http3 mtls required\n" } else { "http3 mtls optional\n" };
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: {},\n                ca_cert_path: {:?},\n            )),\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/v3\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        mode,
        ca_path.display().to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}
