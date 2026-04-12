from __future__ import annotations

import pathlib
import textwrap

BENCHMARK_WORKERS = 4
BENCHMARK_UPSTREAM_KEEPALIVE = 256


def rginx_return_config(port: int) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ok\\n"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_proxy_config(port: int, upstream_port: int) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [
                UpstreamConfig(
                    name: "backend",
                    peers: [UpstreamPeerConfig(url: "http://127.0.0.1:{upstream_port}")],
                    protocol: Http1,
                    load_balance: RoundRobin,
                    pool_max_idle_per_host: Some({BENCHMARK_UPSTREAM_KEEPALIVE}),
                ),
            ],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Prefix("/"),
                    handler: Proxy(upstream: "backend"),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_tls_return_config(port: int, cert_path: pathlib.Path, key_path: pathlib.Path) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
                tls: Some(ServerTlsConfig(
                    cert_path: "{cert_path}",
                    key_path: "{key_path}",
                )),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ok\\n"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_http3_return_config(port: int, cert_path: pathlib.Path, key_path: pathlib.Path) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
                server_names: ["localhost"],
                tls: Some(ServerTlsConfig(
                    cert_path: "{cert_path}",
                    key_path: "{key_path}",
                )),
                http3: Some(Http3Config(
                    advertise_alt_svc: Some(true),
                    alt_svc_max_age_secs: Some(7200),
                )),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ok\\n"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )
    

def rginx_grpc_proxy_config(
    port: int,
    cert_path: pathlib.Path,
    key_path: pathlib.Path,
    backend_port: int,
) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
                tls: Some(ServerTlsConfig(
                    cert_path: "{cert_path}",
                    key_path: "{key_path}",
                )),
            ),
            upstreams: [
                UpstreamConfig(
                    name: "grpc-backend",
                    peers: [UpstreamPeerConfig(url: "https://localhost:{backend_port}")],
                    tls: Some(Insecure),
                    protocol: Http2,
                ),
            ],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Prefix("/"),
                    handler: Proxy(upstream: "grpc-backend"),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_reload_config(port: int, body: str) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some({BENCHMARK_WORKERS}),
                accept_workers: Some({BENCHMARK_WORKERS}),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("{body}"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def nginx_return_config(port: int) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes {BENCHMARK_WORKERS};
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "ok\\n";
                }}
            }}
        }}
        """
    )


def nginx_tls_return_config(port: int, cert_path: pathlib.Path, key_path: pathlib.Path) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes {BENCHMARK_WORKERS};
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port} ssl http2;
                ssl_certificate {cert_path};
                ssl_certificate_key {key_path};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "ok\\n";
                }}
            }}
        }}
        """
    )


def nginx_grpc_proxy_config(
    port: int,
    cert_path: pathlib.Path,
    key_path: pathlib.Path,
    backend_port: int,
) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes {BENCHMARK_WORKERS};
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port} ssl http2;
                ssl_certificate {cert_path};
                ssl_certificate_key {key_path};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = /bench.Bench/Ping {{
                    grpc_ssl_name localhost;
                    grpc_ssl_verify off;
                    grpc_pass grpcs://localhost:{backend_port};
                }}
            }}
        }}
        """
    )


def nginx_reload_config(port: int, body: str) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes {BENCHMARK_WORKERS};
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "{body}";
                }}
            }}
        }}
        """
    )


def nginx_proxy_config(port: int, upstream_port: int) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes {BENCHMARK_WORKERS};
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location / {{
                    proxy_http_version 1.1;
                    proxy_set_header Connection "";
                    proxy_set_header Host $host;
                    proxy_pass http://127.0.0.1:{upstream_port};
                }}
            }}
        }}
        """
    )


def grpc_frame(payload: bytes) -> bytes:
    return bytes([0]) + len(payload).to_bytes(4, byteorder="big") + payload
