use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use proptest::prelude::*;

use super::{expand_env_placeholders_in_ron_strings, load_from_path, load_from_str};

#[test]
fn load_from_str_deserializes_cache_zones_and_route_cache_policy() {
    let config = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    cache_zones: [\n        CacheZoneConfig(\n            name: \"default\",\n            path: \"/var/cache/rginx/default\",\n            max_size_bytes: Some(1048576),\n            inactive_secs: Some(600),\n            default_ttl_secs: Some(60),\n            max_entry_bytes: Some(65536),\n        ),\n    ],\n    server: ServerConfig(\n        listen: \"127.0.0.1:18080\",\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: \"http://127.0.0.1:9000\")],\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/assets/\"),\n            handler: Proxy(upstream: \"backend\"),\n            cache: Some(CacheRouteConfig(\n                zone: \"default\",\n                methods: Some([\"GET\", \"HEAD\"]),\n                statuses: Some([200, 404]),\n                key: Some(\"{scheme}:{host}:{uri}\"),\n                stale_if_error_secs: Some(30),\n            )),\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect("cache config should deserialize");

    assert_eq!(config.cache_zones.len(), 1);
    assert_eq!(config.cache_zones[0].name, "default");
    assert_eq!(config.cache_zones[0].inactive_secs, Some(600));
    assert_eq!(config.cache_zones[0].default_ttl_secs, Some(60));
    assert_eq!(config.cache_zones[0].max_entry_bytes, Some(65536));

    let cache = config.locations[0].cache.as_ref().expect("route cache should deserialize");
    assert_eq!(cache.zone, "default");
    assert_eq!(cache.methods.as_deref(), Some(&["GET".to_string(), "HEAD".to_string()][..]));
    assert_eq!(cache.statuses.as_deref(), Some(&[200, 404][..]));
    assert_eq!(cache.key.as_deref(), Some("{scheme}:{host}:{uri}"));
    assert_eq!(cache.stale_if_error_secs, Some(30));
}

#[test]
fn load_from_str_expands_environment_placeholders_inside_strings() {
    let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::set_var("rginx_test_listen", "127.0.0.1:19090");
        std::env::set_var("rginx_test_body", "hello \"env\"\n");
    }

    let config = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_listen}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"${rginx_test_body}\"),\n            ),\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect("config should load with env expansion");

    assert_eq!(config.server.listen.as_deref(), Some("127.0.0.1:19090"));
    match &config.locations[0].handler {
        crate::model::HandlerConfig::Return { body, .. } => {
            assert_eq!(body.as_deref(), Some("hello \"env\"\n"));
        }
        _ => panic!("expected return handler"),
    }

    unsafe {
        std::env::remove_var("rginx_test_listen");
        std::env::remove_var("rginx_test_body");
    }
}

#[test]
fn load_from_str_supports_env_defaults_and_literal_dollar_escape() {
    let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::remove_var("rginx_test_missing");
    }

    let config = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_missing:-127.0.0.1:18080}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"$${rginx_test_missing}\"),\n            ),\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect("config should load with env defaults");

    assert_eq!(config.server.listen.as_deref(), Some("127.0.0.1:18080"));
    match &config.locations[0].handler {
        crate::model::HandlerConfig::Return { body, .. } => {
            assert_eq!(body.as_deref(), Some("${rginx_test_missing}"));
        }
        _ => panic!("expected return handler"),
    }
}

#[test]
fn load_from_str_supports_legacy_and_structured_upstream_tls_config() {
    let config = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18080\",\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"legacy\",\n            peers: [UpstreamPeerConfig(url: \"https://legacy.example.com\")],\n            tls: Some(Insecure),\n        ),\n        UpstreamConfig(\n            name: \"structured\",\n            peers: [UpstreamPeerConfig(url: \"https://structured.example.com\")],\n            tls: Some(UpstreamTlsConfig(\n                verify: CustomCa(ca_cert_path: \"ca.pem\"),\n                versions: Some([Tls13]),\n                client_cert_path: Some(\"client.crt\"),\n                client_key_path: Some(\"client.key\"),\n            )),\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(upstream: \"legacy\"),\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect("TLS config variants should deserialize");

    let legacy = config.upstreams[0].tls.as_ref().expect("legacy TLS should exist");
    assert!(matches!(legacy.verify, crate::model::UpstreamTlsModeConfig::Insecure));
    assert!(legacy.versions.is_none());

    let structured = config.upstreams[1].tls.as_ref().expect("structured TLS should exist");
    assert_eq!(structured.client_cert_path.as_deref(), Some("client.crt"));
    assert_eq!(structured.client_key_path.as_deref(), Some("client.key"));
    assert!(matches!(
        structured.versions.as_deref(),
        Some([crate::model::TlsVersionConfig::Tls13])
    ));
    assert!(matches!(structured.verify, crate::model::UpstreamTlsModeConfig::CustomCa { .. }));
}

#[test]
fn load_from_str_supports_nezha_dashboard_native_config_shape() {
    let config = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 10,\n    ),\n    server: ServerConfig(\n        trusted_proxies: [\"0.0.0.0/0\", \"::/0\"],\n        client_ip_header: Some(\"CF-Connecting-IP\"),\n    ),\n    upstreams: [],\n    locations: [],\n    servers: [\n        VirtualHostConfig(\n            listen: [\"0.0.0.0:443 ssl http2\", \"[::]:443 ssl http2\"],\n            server_names: [\"dashboard.example.com\"],\n            tls: Some(VirtualHostTlsConfig(\n                cert_path: \"/data/letsencrypt/fullchain.pem\",\n                key_path: \"/data/letsencrypt/key.pem\",\n            )),\n            upstreams: [\n                UpstreamConfig(\n                    name: \"dashboard_http\",\n                    peers: [UpstreamPeerConfig(url: \"http://127.0.0.1:8008\")],\n                    pool_max_idle_per_host: Some(512),\n                    read_timeout_secs: Some(3600),\n                    write_timeout_secs: Some(3600),\n                ),\n                UpstreamConfig(\n                    name: \"dashboard_grpc\",\n                    peers: [UpstreamPeerConfig(url: \"http://127.0.0.1:8008\")],\n                    protocol: H2c,\n                    pool_max_idle_per_host: Some(512),\n                    read_timeout_secs: Some(600),\n                    write_timeout_secs: Some(600),\n                ),\n            ],\n            locations: [\n                LocationConfig(\n                    matcher: Prefix(\"/proto.NezhaService/\"),\n                    handler: Proxy(\n                        upstream: \"dashboard_grpc\",\n                        preserve_host: Some(true),\n                        proxy_set_headers: {\n                            \"nz-realip\": ClientIp,\n                        },\n                    ),\n                ),\n                LocationConfig(\n                    matcher: Regex(\n                        pattern: \"^/api/v1/ws/(server|terminal|file)(/.*)?$\",\n                        case_insensitive: true,\n                    ),\n                    handler: Proxy(\n                        upstream: \"dashboard_http\",\n                        preserve_host: Some(true),\n                        proxy_set_headers: {\n                            \"nz-realip\": ClientIp,\n                            \"origin\": Template(\"https://{host}\"),\n                        },\n                    ),\n                    response_buffering: Some(Off),\n                    compression: Some(Off),\n                ),\n                LocationConfig(\n                    matcher: Prefix(\"/\"),\n                    handler: Proxy(\n                        upstream: \"dashboard_http\",\n                        preserve_host: Some(true),\n                        proxy_set_headers: {\n                            \"nz-realip\": ClientIp,\n                        },\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect("Nezha dashboard native config should deserialize");

    assert_eq!(config.server.client_ip_header.as_deref(), Some("CF-Connecting-IP"));
    assert_eq!(config.servers.len(), 1);
    assert_eq!(config.servers[0].listen.len(), 2);
    assert!(matches!(
        config.servers[0].upstreams[1].protocol,
        crate::model::UpstreamProtocolConfig::H2c
    ));
    assert!(matches!(
        config.servers[0].locations[1].matcher,
        crate::model::MatcherConfig::Regex { case_insensitive: true, .. }
    ));
    let crate::model::HandlerConfig::Proxy { proxy_set_headers, .. } =
        &config.servers[0].locations[1].handler
    else {
        panic!("WebSocket route should proxy");
    };
    assert!(matches!(
        proxy_set_headers.get("nz-realip"),
        Some(crate::model::ProxyHeaderValueConfig::Dynamic(
            crate::model::ProxyHeaderDynamicValueConfig::ClientIp
        ))
    ));
    assert!(matches!(
        proxy_set_headers.get("origin"),
        Some(crate::model::ProxyHeaderValueConfig::Dynamic(
            crate::model::ProxyHeaderDynamicValueConfig::Template(template)
        )) if template == "https://{host}"
    ));
}

#[test]
fn load_from_str_rejects_missing_environment_placeholders() {
    let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::remove_var("rginx_test_required");
    }

    let error = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_test_required}\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        Path::new("inline.ron"),
    )
    .expect_err("missing env placeholder should fail");

    assert!(error.to_string().contains("environment variable `rginx_test_required` is not set"));
}

#[test]
fn load_from_path_expands_relative_includes_recursively() {
    let temp_dir = temp_dir("rginx-load-include-test");
    fs::create_dir_all(temp_dir.join("fragments")).expect("temp fragments dir should be created");
    let config_path = temp_dir.join("rginx.ron");
    let routes_path = temp_dir.join("fragments/routes.ron");
    let body_path = temp_dir.join("fragments/body.ron");

    fs::write(&body_path, "\"included body\\n\"").expect("body fragment should be written");
    fs::write(
        &routes_path,
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some(\n            // @include \"body.ron\"\n        ),\n    ),\n),\n",
    )
    .expect("routes fragment should be written");
    fs::write(
        &config_path,
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18081\",\n    ),\n    upstreams: [],\n    locations: [\n        // @include \"fragments/routes.ron\"\n    ],\n)\n",
    )
    .expect("root config should be written");

    let config = load_from_path(&config_path).expect("config with includes should load");
    match &config.locations[0].handler {
        crate::model::HandlerConfig::Return { body, .. } => {
            assert_eq!(body.as_deref(), Some("included body\n"));
        }
        _ => panic!("expected return handler"),
    }

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn load_from_path_rejects_include_cycles() {
    let temp_dir = temp_dir("rginx-load-include-cycle-test");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let first = temp_dir.join("first.ron");
    let second = temp_dir.join("second.ron");

    fs::write(&first, "// @include \"second.ron\"\n").expect("first include should be written");
    fs::write(&second, "// @include \"first.ron\"\n").expect("second include should be written");

    let error = load_from_path(&first).expect_err("include cycle should fail");
    assert!(error.to_string().contains("config include cycle detected"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn load_from_path_expands_sorted_conf_d_glob_fragments() {
    let temp_dir = temp_dir("rginx-load-conf-d-test");
    let conf_d = temp_dir.join("conf.d");
    fs::create_dir_all(&conf_d).expect("conf.d should be created");
    let config_path = temp_dir.join("rginx.ron");

    fs::write(
        conf_d.join("20-app.ron"),
        "VirtualHostConfig(\n    server_names: [\"app.example.com\"],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"app\\n\"),\n            ),\n        ),\n    ],\n),\n",
    )
    .expect("app vhost should be written");
    fs::write(
        conf_d.join("10-api.ron"),
        "VirtualHostConfig(\n    server_names: [\"api.example.com\"],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"api\\n\"),\n            ),\n        ),\n    ],\n),\n",
    )
    .expect("api vhost should be written");
    fs::write(
        &config_path,
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"0.0.0.0:80\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"root\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        // @include \"conf.d/*.ron\"\n    ],\n)\n",
    )
    .expect("root config should be written");

    let config = load_from_path(&config_path).expect("config with conf.d glob should load");

    assert_eq!(config.servers.len(), 2);
    assert_eq!(config.servers[0].server_names, vec!["api.example.com".to_string()]);
    assert_eq!(config.servers[1].server_names, vec!["app.example.com".to_string()]);

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn load_from_path_rejects_unsupported_include_globs() {
    let error = load_from_str(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"0.0.0.0:80\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        // @include \"conf.d/*.txt\"\n    ],\n)\n",
        Path::new("rginx.ron"),
    )
    .expect_err("unsupported glob should fail");

    assert!(error.to_string().contains("only `*.ron` file globs are supported"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn env_placeholder_expansion_leaves_placeholder_free_sources_unchanged(
        source in prop::collection::vec(
            any::<char>().prop_filter("source must not contain `$`", |ch| *ch != '$'),
            0..128,
        )
        .prop_map(|chars| chars.into_iter().collect::<String>())
    ) {
        let expanded = expand_env_placeholders_in_ron_strings(&source, Path::new("inline.ron"))
            .expect("placeholder-free source should not fail");

        prop_assert_eq!(expanded, source);
    }

    #[test]
    fn load_from_str_round_trips_arbitrary_env_string_values(value in arbitrary_env_value()) {
        let _guard = env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _scoped_env = ScopedEnvVar::set("rginx_test_prop_body", &value);

        let config = load_from_str(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:18080\",\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"${rginx_test_prop_body}\"),\n            ),\n        ),\n    ],\n)\n",
            Path::new("inline.ron"),
        )
        .expect("config should load with arbitrary env expansion");

        match &config.locations[0].handler {
            crate::model::HandlerConfig::Return { body, .. } => {
                prop_assert_eq!(body.as_deref(), Some(value.as_str()));
            }
            _ => panic!("expected return handler"),
        }
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{unique}"))
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn arbitrary_env_value() -> impl Strategy<Value = String> {
    prop::collection::vec(
        any::<char>().prop_filter("exclude unescaped control chars invalid in RON strings", |ch| {
            (*ch >= ' ' && *ch != '\x7F') || matches!(*ch, '\n' | '\r' | '\t')
        }),
        0..64,
    )
    .prop_map(|chars| chars.into_iter().collect::<String>())
}

struct ScopedEnvVar {
    key: &'static str,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var(self.key);
        }
    }
}
