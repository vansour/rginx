use super::*;

fn cache_zone(name: &str) -> CacheZoneConfig {
    CacheZoneConfig {
        name: name.to_string(),
        path: format!("/tmp/rginx-cache/{name}"),
        max_size_bytes: Some(1024 * 1024),
        inactive_secs: Some(60),
        default_ttl_secs: Some(30),
        max_entry_bytes: Some(1024),
    }
}

fn route_cache(zone: &str) -> CacheRouteConfig {
    CacheRouteConfig {
        zone: zone.to_string(),
        methods: Some(vec!["GET".to_string(), "HEAD".to_string()]),
        statuses: Some(vec![200, 301, 404]),
        key: Some("{scheme}:{host}:{uri}".to_string()),
        stale_if_error_secs: Some(10),
    }
}

#[test]
fn validate_accepts_cache_zone_and_proxy_route_policy() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    config.locations[0].cache = Some(route_cache("default"));

    validate(&config).expect("valid cache config should pass validation");
}

#[test]
fn validate_rejects_route_cache_with_undefined_zone() {
    let mut config = base_config();
    config.locations[0].cache = Some(route_cache("missing"));

    let error = validate(&config).expect_err("undefined cache zone should fail validation");
    assert!(error.to_string().contains("references undefined cache zone `missing`"), "{error}");
}

#[test]
fn validate_rejects_cache_policy_on_return_route() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    config.locations[0].handler = HandlerConfig::Return {
        status: 200,
        location: String::new(),
        body: Some("ok\n".to_string()),
    };
    config.locations[0].cache = Some(route_cache("default"));

    let error = validate(&config).expect_err("return route cache should fail validation");
    assert!(error.to_string().contains("cache requires a Proxy handler"), "{error}");
}

#[test]
fn validate_rejects_unsupported_cache_method_and_invalid_zone_limits() {
    let mut config = base_config();
    config.cache_zones = vec![CacheZoneConfig {
        max_entry_bytes: Some(2 * 1024),
        max_size_bytes: Some(1024),
        ..cache_zone("default")
    }];
    config.locations[0].cache = Some(route_cache("default"));

    let error = validate(&config).expect_err("oversized max_entry should fail validation");
    assert!(error.to_string().contains("max_entry_bytes must not exceed max_size_bytes"));

    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.methods = Some(vec!["POST".to_string()]);
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("unsupported cache method should fail validation");
    assert!(error.to_string().contains("cache method `POST` is not supported"));
}
