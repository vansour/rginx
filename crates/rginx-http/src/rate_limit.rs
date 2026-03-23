use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rginx_core::RouteRateLimit;

#[derive(Clone, Default)]
pub struct RateLimiters {
    inner: Arc<Mutex<HashMap<BucketKey, TokenBucket>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    route: String,
    client_ip: IpAddr,
}

#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiters {
    pub fn check(&self, route: &str, client_ip: IpAddr, policy: Option<&RouteRateLimit>) -> bool {
        let Some(policy) = policy.copied() else {
            return true;
        };

        let now = Instant::now();
        let mut buckets = lock_map(&self.inner);
        let bucket = buckets
            .entry(BucketKey { route: route.to_string(), client_ip })
            .or_insert_with(|| TokenBucket::new(policy, now));

        bucket.try_acquire(policy, now)
    }
}

impl TokenBucket {
    fn new(policy: RouteRateLimit, now: Instant) -> Self {
        Self { tokens: bucket_capacity(policy), last_refill: now }
    }

    fn try_acquire(&mut self, policy: RouteRateLimit, now: Instant) -> bool {
        let replenished = self.tokens
            + now.duration_since(self.last_refill).as_secs_f64() * policy.requests_per_sec as f64;
        self.tokens = replenished.min(bucket_capacity(policy));
        self.last_refill = now;

        if self.tokens < 1.0 {
            return false;
        }

        self.tokens -= 1.0;
        true
    }
}

fn bucket_capacity(policy: RouteRateLimit) -> f64 {
    (policy.burst + 1) as f64
}

fn lock_map<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
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
}
