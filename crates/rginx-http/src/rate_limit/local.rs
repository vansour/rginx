use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use rginx_core::RouteRateLimit;

const MIN_SHARD_COUNT: usize = 4;
const MAX_SHARD_COUNT: usize = 64;
const SHARD_CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct LocalRateLimitersInner {
    pub(super) shard_mask: usize,
    pub(super) cleanup_interval: Duration,
    pub(super) shards: Box<[Shard]>,
}

pub(super) struct LocalRateLimiters {
    pub(super) inner: LocalRateLimitersInner,
}

pub(super) struct Shard {
    pub(super) state: Mutex<ShardState>,
}

pub(super) struct ShardState {
    pub(super) buckets: HashMap<BucketKey, TokenBucket>,
    pub(super) next_cleanup_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct BucketKey {
    pub(super) route: String,
    pub(super) client_ip: IpAddr,
}

#[derive(Debug, Clone)]
pub(super) struct TokenBucket {
    policy: RouteRateLimit,
    tokens: f64,
    last_refill: Instant,
}

impl Default for LocalRateLimiters {
    fn default() -> Self {
        Self::with_config(default_shard_count(), SHARD_CLEANUP_INTERVAL)
    }
}

impl LocalRateLimiters {
    pub(super) fn check(&self, route: &str, client_ip: IpAddr, policy: RouteRateLimit) -> bool {
        self.check_at(route, client_ip, policy, Instant::now())
    }

    pub(super) fn check_at(
        &self,
        route: &str,
        client_ip: IpAddr,
        policy: RouteRateLimit,
        now: Instant,
    ) -> bool {
        let shard = self.inner.shard(route, client_ip);
        let mut state = lock_map(&shard.state);
        maybe_cleanup_buckets(&mut state, now, self.inner.cleanup_interval);

        let bucket = state
            .buckets
            .entry(BucketKey { route: route.to_string(), client_ip })
            .or_insert_with(|| TokenBucket::new(policy, now));

        bucket.try_acquire(policy, now)
    }

    pub(super) fn with_config(shard_count: usize, cleanup_interval: Duration) -> Self {
        let shard_count = shard_count.max(1).next_power_of_two();
        let now = Instant::now();
        let next_cleanup_at = now + cleanup_interval;
        let shards = (0..shard_count)
            .map(|_| Shard::new(next_cleanup_at))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self {
            inner: LocalRateLimitersInner { shard_mask: shard_count - 1, cleanup_interval, shards },
        }
    }
}

impl LocalRateLimitersInner {
    fn shard(&self, route: &str, client_ip: IpAddr) -> &Shard {
        &self.shards[self.shard_index(route, client_ip)]
    }

    fn shard_index(&self, route: &str, client_ip: IpAddr) -> usize {
        let mut hasher = DefaultHasher::new();
        route.hash(&mut hasher);
        client_ip.hash(&mut hasher);
        (hasher.finish() as usize) & self.shard_mask
    }
}

impl Shard {
    fn new(next_cleanup_at: Instant) -> Self {
        Self { state: Mutex::new(ShardState { buckets: HashMap::new(), next_cleanup_at }) }
    }
}

impl TokenBucket {
    pub(super) fn new(policy: RouteRateLimit, now: Instant) -> Self {
        Self { policy, tokens: bucket_capacity(policy), last_refill: now }
    }

    pub(super) fn try_acquire(&mut self, policy: RouteRateLimit, now: Instant) -> bool {
        self.reconfigure(policy, now);
        self.refill(now);

        if self.tokens < 1.0 {
            return false;
        }

        self.tokens -= 1.0;
        true
    }

    pub(super) fn is_evictable(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= bucket_capacity(self.policy)
    }

    fn reconfigure(&mut self, policy: RouteRateLimit, now: Instant) {
        if self.policy == policy {
            return;
        }

        self.refill(now);
        self.tokens = self.tokens.min(bucket_capacity(policy));
        self.policy = policy;
    }

    fn refill(&mut self, now: Instant) {
        let replenished = self.tokens
            + now.duration_since(self.last_refill).as_secs_f64()
                * self.policy.requests_per_sec as f64;
        self.tokens = replenished.min(bucket_capacity(self.policy));
        self.last_refill = now;
    }
}

pub(super) fn bucket_capacity(policy: RouteRateLimit) -> f64 {
    (policy.burst + 1) as f64
}

fn maybe_cleanup_buckets(state: &mut ShardState, now: Instant, cleanup_interval: Duration) {
    if now < state.next_cleanup_at {
        return;
    }

    state.buckets.retain(|_, bucket| !bucket.is_evictable(now));
    state.next_cleanup_at = now + cleanup_interval;
}

fn default_shard_count() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().next_power_of_two())
        .unwrap_or(MIN_SHARD_COUNT)
        .clamp(MIN_SHARD_COUNT, MAX_SHARD_COUNT)
}

pub(super) fn lock_map<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
