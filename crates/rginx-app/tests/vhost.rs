use std::net::SocketAddr;
use std::time::Duration;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn routes_requests_by_host_and_path_end_to_end() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-vhost-test", |_| vhost_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        "default.example.com",
        "/",
        200,
        "default root\n",
        Duration::from_secs(5),
    );
    server.wait_for_http_text_response(
        listen_addr,
        "unknown.example.com",
        "/",
        200,
        "default root\n",
        Duration::from_secs(5),
    );
    server.wait_for_http_text_response(
        listen_addr,
        "api.example.com",
        "/users",
        200,
        "api users\n",
        Duration::from_secs(5),
    );
    server.wait_for_http_text_response(
        listen_addr,
        &format!("api.example.com:{}", listen_addr.port()),
        "/",
        200,
        "api root\n",
        Duration::from_secs(5),
    );
    server.wait_for_http_text_response(
        listen_addr,
        "app.internal.example.com",
        "/",
        200,
        "internal root\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn matched_vhost_does_not_fall_back_to_default_routes_end_to_end() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-vhost-test", |_| no_fallback_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        "default.example.com",
        "/users",
        200,
        "default users\n",
        Duration::from_secs(5),
    );
    server.wait_for_http_text_response(
        listen_addr,
        "api.example.com",
        "/users",
        404,
        "route not found\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
fn vhost_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default root\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api root\\n\"),\n                    ),\n                ),\n                LocationConfig(\n                    matcher: Exact(\"/users\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api users\\n\"),\n                    ),\n                ),\n            ],\n        ),\n        VirtualHostConfig(\n            server_names: [\"*.internal.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"internal root\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn no_fallback_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/users\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default users\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/status\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api status\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}
