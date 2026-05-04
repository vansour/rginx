use std::net::IpAddr;
use std::time::{Duration, Instant};

use rginx_core::RouteRateLimit;

use super::RateLimiters;
use super::local::bucket_capacity;

#[test]
fn token_bucket_blocks_after_burst_is_exhausted() {
    let limiters = RateLimiters::with_local_config(1, Duration::from_secs(30));
    let policy = RouteRateLimit::new(2, 1);
    let now = Instant::now();

    assert!(limiters.local.check_at("route", "192.0.2.10".parse().unwrap(), policy, now));
    assert!(limiters.local.check_at("route", "192.0.2.10".parse().unwrap(), policy, now));
    assert!(!limiters.local.check_at("route", "192.0.2.10".parse().unwrap(), policy, now));
}

#[test]
fn token_bucket_refills_over_time() {
    let limiters = RateLimiters::with_local_config(1, Duration::from_secs(30));
    let policy = RouteRateLimit::new(2, 0);
    let now = Instant::now();

    assert!(limiters.local.check_at("route", "192.0.2.10".parse().unwrap(), policy, now));
    assert!(!limiters.local.check_at("route", "192.0.2.10".parse().unwrap(), policy, now));
    assert!(limiters.local.check_at(
        "route",
        "192.0.2.10".parse().unwrap(),
        policy,
        now + Duration::from_millis(500),
    ));
}

#[test]
fn rate_limiters_isolate_buckets_by_route_and_ip() {
    let limiters = RateLimiters::local_only();
    let policy = RouteRateLimit::new(1, 0);
    let first_ip: IpAddr = "192.0.2.10".parse().unwrap();
    let second_ip: IpAddr = "192.0.2.11".parse().unwrap();

    assert!(limiters.check("server/routes[0]|prefix:/api", first_ip, Some(&policy)));
    assert!(!limiters.check("server/routes[0]|prefix:/api", first_ip, Some(&policy)));
    assert!(limiters.check("server/routes[0]|prefix:/api", second_ip, Some(&policy)));
    assert!(limiters.check("servers[0]/routes[0]|exact:/status", first_ip, Some(&policy)));
}

#[test]
fn bucket_capacity_includes_the_current_request() {
    assert_eq!(bucket_capacity(RouteRateLimit::new(10, 0)), 1.0);
    assert_eq!(bucket_capacity(RouteRateLimit::new(10, 3)), 4.0);
}

#[test]
fn cleanup_evicts_buckets_that_have_fully_refilled() {
    let limiters = RateLimiters::with_local_config(1, Duration::ZERO);
    let policy = RouteRateLimit::new(2, 0);
    let now = Instant::now();
    let first_ip: IpAddr = "192.0.2.10".parse().unwrap();
    let second_ip: IpAddr = "192.0.2.11".parse().unwrap();
    let third_ip: IpAddr = "192.0.2.12".parse().unwrap();

    assert!(limiters.local.check_at("route-a", first_ip, policy, now));
    assert!(limiters.local.check_at("route-b", second_ip, policy, now));
    assert_eq!(total_bucket_count(&limiters), 2);

    assert!(limiters.local.check_at("route-c", third_ip, policy, now + Duration::from_secs(1)));
    assert_eq!(total_bucket_count(&limiters), 1);
}

#[test]
fn cleanup_keeps_buckets_that_have_not_refilled_yet() {
    let limiters = RateLimiters::with_local_config(1, Duration::ZERO);
    let policy = RouteRateLimit::new(1, 0);
    let now = Instant::now();
    let first_ip: IpAddr = "192.0.2.10".parse().unwrap();
    let second_ip: IpAddr = "192.0.2.11".parse().unwrap();

    assert!(limiters.local.check_at("route-a", first_ip, policy, now));
    assert_eq!(total_bucket_count(&limiters), 1);

    assert!(limiters.local.check_at(
        "route-b",
        second_ip,
        policy,
        now + Duration::from_millis(500),
    ));
    assert_eq!(total_bucket_count(&limiters), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn shared_rate_limiters_share_buckets_across_instances() {
    let identity = format!("rate-limit-shared-{}", std::process::id());
    let limiter_a = RateLimiters::with_shared_identity(&identity);
    let limiter_b = RateLimiters::with_shared_identity(&identity);
    let policy = RouteRateLimit::new(1, 0);
    let client_ip: IpAddr = "192.0.2.10".parse().unwrap();

    limiter_a
        .shared
        .as_ref()
        .expect("shared store should exist")
        .unlink_for_test()
        .expect("test shm state should reset");

    assert!(limiter_a.check_shared_at("route", client_ip, policy, 1_000));
    assert!(!limiter_b.check_shared_at("route", client_ip, policy, 1_000));

    limiter_a
        .shared
        .as_ref()
        .expect("shared store should exist")
        .unlink_for_test()
        .expect("test shm state should unlink");
}

#[cfg(target_os = "linux")]
#[test]
fn shared_rate_limiters_refill_over_time() {
    let identity = format!("rate-limit-refill-{}", std::process::id());
    let limiters = RateLimiters::with_shared_identity(&identity);
    let policy = RouteRateLimit::new(2, 0);
    let client_ip: IpAddr = "192.0.2.10".parse().unwrap();

    limiters
        .shared
        .as_ref()
        .expect("shared store should exist")
        .unlink_for_test()
        .expect("test shm state should reset");

    assert!(limiters.check_shared_at("route", client_ip, policy, 1_000));
    assert!(!limiters.check_shared_at("route", client_ip, policy, 1_000));
    assert!(limiters.check_shared_at("route", client_ip, policy, 1_500));

    limiters
        .shared
        .as_ref()
        .expect("shared store should exist")
        .unlink_for_test()
        .expect("test shm state should unlink");
}

fn total_bucket_count(limiters: &RateLimiters) -> usize {
    limiters
        .local
        .inner
        .shards
        .iter()
        .map(|shard| super::local::lock_map(&shard.state).buckets.len())
        .sum()
}
