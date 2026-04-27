# Nezha Dashboard Proxy Example

This example shows the rginx-native way to proxy a Nezha dashboard behind an
HTTPS HTTP/2 frontend. It does not parse nginx syntax; the same behavior is
expressed with rginx RON config.

## Global Defaults

Keep listener-wide trust policy in `configs/rginx.ron`:

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        trusted_proxies: ["0.0.0.0/0", "::/0"], // Prefer concrete CDN CIDRs in production.
        client_ip_header: Some("CF-Connecting-IP"),
    ),
    upstreams: [],
    locations: [],
    servers: [
        // @include "conf.d/*.ron"
    ],
)
```

`client_ip_header` is only trusted when the socket peer is covered by
`trusted_proxies`. The resolved client IP is then available to proxy headers
through `ClientIp`.

## Site Config

Place the site in `configs/conf.d/dashboard.ron`:

```ron
VirtualHostConfig(
    listen: ["0.0.0.0:443 ssl http2", "[::]:443 ssl http2"],
    server_names: ["dashboard.example.com"],
    tls: Some(VirtualHostTlsConfig(
        cert_path: "/data/letsencrypt/fullchain.pem",
        key_path: "/data/letsencrypt/key.pem",
    )),
    upstreams: [
        UpstreamConfig(
            name: "dashboard_http",
            peers: [UpstreamPeerConfig(url: "http://127.0.0.1:8008")],
            pool_max_idle_per_host: Some(512),
            read_timeout_secs: Some(3600),
            write_timeout_secs: Some(3600),
        ),
        UpstreamConfig(
            name: "dashboard_grpc",
            peers: [UpstreamPeerConfig(url: "http://127.0.0.1:8008")],
            protocol: H2c,
            pool_max_idle_per_host: Some(512),
            read_timeout_secs: Some(600),
            write_timeout_secs: Some(600),
            http2_keep_alive_interval_secs: Some(30),
            http2_keep_alive_timeout_secs: Some(10),
            http2_keep_alive_while_idle: Some(true),
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Prefix("/proto.NezhaService/"),
            handler: Proxy(
                upstream: "dashboard_grpc",
                preserve_host: Some(true),
                proxy_set_headers: {
                    "nz-realip": ClientIp,
                },
            ),
        ),
        LocationConfig(
            matcher: Regex(
                pattern: "^/api/v1/ws/(server|terminal|file)(/.*)?$",
                case_insensitive: true,
            ),
            handler: Proxy(
                upstream: "dashboard_http",
                preserve_host: Some(true),
                proxy_set_headers: {
                    "nz-realip": ClientIp,
                    "origin": Template("https://{host}"),
                },
            ),
            response_buffering: Some(Off),
            compression: Some(Off),
        ),
        LocationConfig(
            matcher: Prefix("/"),
            handler: Proxy(
                upstream: "dashboard_http",
                preserve_host: Some(true),
                proxy_set_headers: {
                    "nz-realip": ClientIp,
                },
            ),
            response_buffering: Some(On),
        ),
    ],
)
```

## Notes

- `H2c` is the rginx upstream protocol for cleartext HTTP/2 gRPC backends, matching nginx `grpc://` behavior.
- `Regex(...)` is a native rginx matcher, not nginx `location ~*` syntax.
- `proxy_set_headers` supports dynamic values such as `ClientIp`, `Host`, `Scheme`, `RequestHeader("name")`, and `Template("https://{host}")`.
- WebSocket Upgrade headers are preserved automatically; they do not need to be configured manually.
- nginx buffer directives such as `proxy_buffers` and `grpc_buffer_size` are not copied 1:1. Use rginx buffering, streaming, timeout, and request-size controls instead.
