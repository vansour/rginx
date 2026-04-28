use super::*;

fn cache_zone(name: &str) -> CacheZoneConfig {
    CacheZoneConfig {
        name: name.to_string(),
        path: format!("/tmp/rginx-cache/{name}"),
        max_size_bytes: Some(1024 * 1024),
        inactive_secs: Some(60),
        default_ttl_secs: Some(30),
        max_entry_bytes: Some(1024),
        path_levels: None,
        loader_batch_entries: None,
        loader_sleep_millis: None,
        manager_batch_entries: None,
        manager_sleep_millis: None,
        inactive_cleanup_interval_secs: None,
    }
}

fn route_cache(zone: &str) -> CacheRouteConfig {
    CacheRouteConfig {
        zone: zone.to_string(),
        methods: Some(vec!["GET".to_string(), "HEAD".to_string()]),
        statuses: Some(vec![200, 301, 404]),
        ttl_secs_by_status: None,
        key: Some("{scheme}:{host}:{uri}".to_string()),
        cache_bypass: None,
        no_cache: None,
        stale_if_error_secs: Some(10),
        use_stale: None,
        background_update: None,
        lock_timeout_secs: None,
        lock_age_secs: None,
        min_uses: None,
        ignore_headers: None,
        range_requests: None,
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
fn validate_rejects_max_entry_bytes_exceeding_max_size_bytes() {
    let mut config = base_config();
    config.cache_zones = vec![CacheZoneConfig {
        max_entry_bytes: Some(2 * 1024),
        max_size_bytes: Some(1024),
        ..cache_zone("default")
    }];
    config.locations[0].cache = Some(route_cache("default"));

    let error = validate(&config).expect_err("oversized max_entry should fail validation");
    assert!(error.to_string().contains("max_entry_bytes must not exceed max_size_bytes"));
}

#[test]
fn validate_rejects_unsupported_cache_method() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.methods = Some(vec!["POST".to_string()]);
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("unsupported cache method should fail validation");
    assert!(error.to_string().contains("cache method `POST` is not supported"));
}

#[test]
fn validate_rejects_head_only_cache_methods() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.methods = Some(vec!["HEAD".to_string()]);
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("HEAD-only cache methods should fail validation");
    assert!(error.to_string().contains("cache.methods must include GET"), "{error}");
}

#[test]
fn validate_rejects_status_match_in_cache_bypass() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.cache_bypass = Some(crate::model::CachePredicateConfig::Status(200));
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("status-based bypass should fail validation");
    assert!(error.to_string().contains("cache.cache_bypass cannot match response status"));
}

#[test]
fn validate_accepts_status_match_in_no_cache() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.no_cache = Some(crate::model::CachePredicateConfig::Status(200));
    config.locations[0].cache = Some(policy);

    validate(&config).expect("status-based no_cache should pass validation");
}

#[test]
fn validate_rejects_zero_min_uses() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.min_uses = Some(0);
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("zero min_uses should fail validation");
    assert!(error.to_string().contains("cache.min_uses must be greater than 0"), "{error}");
}

#[test]
fn validate_rejects_empty_ignore_headers() {
    let mut config = base_config();
    config.cache_zones = vec![cache_zone("default")];
    let mut policy = route_cache("default");
    policy.ignore_headers = Some(Vec::new());
    config.locations[0].cache = Some(policy);

    let error = validate(&config).expect_err("empty ignore_headers should fail validation");
    assert!(error.to_string().contains("cache.ignore_headers must not be empty"), "{error}");
}

#[test]
fn validate_rejects_empty_path_levels() {
    let mut config = base_config();
    config.cache_zones =
        vec![CacheZoneConfig { path_levels: Some(Vec::new()), ..cache_zone("default") }];
    config.locations[0].cache = Some(route_cache("default"));

    let error = validate(&config).expect_err("empty path_levels should fail validation");
    assert!(error.to_string().contains("path_levels must not be empty"), "{error}");
}
