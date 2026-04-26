use std::net::IpAddr;
use std::time::{Duration, Instant};

use rginx_core::RouteRateLimit;

use super::{RateLimiters, TokenBucket, bucket_capacity};

#[test]
fn token_bucket_blocks_after_burst_is_exhausted() {
    let policy = RouteRateLimit::new(2, 1);
    let now = Instant::now();
    let mut bucket = TokenBucket::new(policy, now);

    assert!(bucket.try_acquire(policy, now));
    assert!(bucket.try_acquire(policy, now));
    assert!(!bucket.try_acquire(policy, now));
}

#[test]
fn token_bucket_refills_over_time() {
    let policy = RouteRateLimit::new(2, 0);
    let now = Instant::now();
    let mut bucket = TokenBucket::new(policy, now);

    assert!(bucket.try_acquire(policy, now));
    assert!(!bucket.try_acquire(policy, now));
    assert!(bucket.try_acquire(policy, now + Duration::from_millis(500)));
}

#[test]
fn rate_limiters_isolate_buckets_by_route_and_ip() {
    let limiters = RateLimiters::default();
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
    let limiters = RateLimiters::with_config(1, Duration::ZERO);
    let policy = RouteRateLimit::new(2, 0);
    let now = Instant::now();
    let first_ip: IpAddr = "192.0.2.10".parse().unwrap();
    let second_ip: IpAddr = "192.0.2.11".parse().unwrap();
    let third_ip: IpAddr = "192.0.2.12".parse().unwrap();

    assert!(limiters.check_at("route-a", first_ip, Some(&policy), now));
    assert!(limiters.check_at("route-b", second_ip, Some(&policy), now));
    assert_eq!(total_bucket_count(&limiters), 2);

    assert!(limiters.check_at("route-c", third_ip, Some(&policy), now + Duration::from_secs(1)));
    assert_eq!(total_bucket_count(&limiters), 1);
}

#[test]
fn cleanup_keeps_buckets_that_have_not_refilled_yet() {
    let limiters = RateLimiters::with_config(1, Duration::ZERO);
    let policy = RouteRateLimit::new(1, 0);
    let now = Instant::now();
    let first_ip: IpAddr = "192.0.2.10".parse().unwrap();
    let second_ip: IpAddr = "192.0.2.11".parse().unwrap();

    assert!(limiters.check_at("route-a", first_ip, Some(&policy), now));
    assert_eq!(total_bucket_count(&limiters), 1);

    assert!(limiters.check_at(
        "route-b",
        second_ip,
        Some(&policy),
        now + Duration::from_millis(500)
    ));
    assert_eq!(total_bucket_count(&limiters), 2);
}

fn total_bucket_count(limiters: &RateLimiters) -> usize {
    limiters.inner.shards.iter().map(|shard| super::lock_map(&shard.state).buckets.len()).sum()
}
