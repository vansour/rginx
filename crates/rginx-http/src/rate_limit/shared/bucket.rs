use rginx_core::RouteRateLimit;
use serde::{Deserialize, Serialize};

const CLEANUP_INTERVAL_MS: u64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedTokenBucket {
    requests_per_sec: u32,
    burst: u32,
    tokens: f64,
    last_refill_unix_ms: u64,
    last_seen_unix_ms: u64,
}

impl SharedTokenBucket {
    pub(super) fn new(policy: RouteRateLimit, now_unix_ms: u64) -> Self {
        Self {
            requests_per_sec: policy.requests_per_sec,
            burst: policy.burst,
            tokens: bucket_capacity(policy),
            last_refill_unix_ms: now_unix_ms,
            last_seen_unix_ms: now_unix_ms,
        }
    }

    pub(super) fn try_acquire(&mut self, policy: RouteRateLimit, now_unix_ms: u64) -> bool {
        self.reconfigure(policy, now_unix_ms);
        self.refill(now_unix_ms);
        self.last_seen_unix_ms = now_unix_ms;

        if self.tokens < 1.0 {
            return false;
        }

        self.tokens -= 1.0;
        true
    }

    fn is_evictable(&mut self, now_unix_ms: u64) -> bool {
        self.refill(now_unix_ms);
        self.tokens >= bucket_capacity(RouteRateLimit::new(self.requests_per_sec, self.burst))
    }

    fn reconfigure(&mut self, policy: RouteRateLimit, now_unix_ms: u64) {
        if self.requests_per_sec == policy.requests_per_sec && self.burst == policy.burst {
            return;
        }

        self.refill(now_unix_ms);
        self.tokens = self.tokens.min(bucket_capacity(policy));
        self.requests_per_sec = policy.requests_per_sec;
        self.burst = policy.burst;
    }

    fn refill(&mut self, now_unix_ms: u64) {
        let elapsed_ms = now_unix_ms.saturating_sub(self.last_refill_unix_ms);
        let replenished =
            self.tokens + (elapsed_ms as f64 / 1_000.0) * self.requests_per_sec as f64;
        self.tokens = replenished
            .min(bucket_capacity(RouteRateLimit::new(self.requests_per_sec, self.burst)));
        self.last_refill_unix_ms = now_unix_ms;
    }
}

pub(super) fn maybe_cleanup_document(
    document: &mut crate::rate_limit::shared::document::SharedRateLimitDocument,
    now_unix_ms: u64,
) {
    if now_unix_ms < document.next_cleanup_unix_ms {
        return;
    }

    document.buckets.retain(|_, bucket| !bucket.is_evictable(now_unix_ms));
    document.next_cleanup_unix_ms = now_unix_ms.saturating_add(CLEANUP_INTERVAL_MS);
}

pub(super) fn bucket_capacity(policy: RouteRateLimit) -> f64 {
    (policy.burst + 1) as f64
}
