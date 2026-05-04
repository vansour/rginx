use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use rginx_core::RouteRateLimit;

mod local;
#[cfg(target_os = "linux")]
mod shared;
#[cfg(not(target_os = "linux"))]
mod shared {}
#[cfg(test)]
mod tests;

use local::LocalRateLimiters;

#[cfg(target_os = "linux")]
use shared::SharedRateLimitStore;

#[derive(Clone)]
pub struct RateLimiters {
    local: Arc<LocalRateLimiters>,
    #[cfg(target_os = "linux")]
    shared: Option<Arc<SharedRateLimitStore>>,
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self::local_only()
    }
}

impl RateLimiters {
    pub fn for_runtime(config_path: Option<&Path>) -> Self {
        let local = Arc::new(LocalRateLimiters::default());
        #[cfg(target_os = "linux")]
        let shared = config_path.and_then(|path| {
            SharedRateLimitStore::new(path).map(Arc::new).map_err(|error| {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to initialize shared rate-limit store; falling back to local limiter state"
                );
            }).ok()
        });

        Self {
            local,
            #[cfg(target_os = "linux")]
            shared,
        }
    }

    pub fn check(&self, route: &str, client_ip: IpAddr, policy: Option<&RouteRateLimit>) -> bool {
        let Some(policy) = policy.copied() else {
            return true;
        };

        #[cfg(target_os = "linux")]
        if let Some(shared) = &self.shared {
            return shared.check(route, client_ip, policy).unwrap_or_else(|error| {
                tracing::warn!(
                    route = route,
                    client_ip = %client_ip,
                    %error,
                    "shared rate-limit check failed; denying request to preserve shared semantics"
                );
                false
            });
        }

        self.local.check(route, client_ip, policy)
    }

    fn local_only() -> Self {
        Self {
            local: Arc::new(LocalRateLimiters::default()),
            #[cfg(target_os = "linux")]
            shared: None,
        }
    }

    #[cfg(test)]
    fn with_local_config(shard_count: usize, cleanup_interval: std::time::Duration) -> Self {
        Self {
            local: Arc::new(LocalRateLimiters::with_config(shard_count, cleanup_interval)),
            #[cfg(target_os = "linux")]
            shared: None,
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    fn with_shared_identity(identity: &str) -> Self {
        Self {
            local: Arc::new(LocalRateLimiters::default()),
            shared: Some(Arc::new(
                SharedRateLimitStore::for_identity(identity)
                    .expect("shared rate-limit store should initialize for tests"),
            )),
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    fn check_shared_at(
        &self,
        route: &str,
        client_ip: IpAddr,
        policy: RouteRateLimit,
        now_unix_ms: u64,
    ) -> bool {
        self.shared
            .as_ref()
            .expect("shared store should exist for shared tests")
            .check_at(route, client_ip, policy, now_unix_ms)
            .expect("shared rate-limit check should succeed in tests")
    }
}
