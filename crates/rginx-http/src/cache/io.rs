use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock};

use super::CacheZoneRuntime;

pub(super) const CACHE_IO_LOCK_STRIPES: usize = 64;

pub(super) struct CacheIoLockPool {
    stripes: Vec<Arc<AsyncRwLock<()>>>,
}

pub(super) struct CacheIoReadGuard {
    _guard: OwnedRwLockReadGuard<()>,
}

pub(super) struct CacheIoWriteGuards {
    _guards: Vec<OwnedRwLockWriteGuard<()>>,
}

impl CacheIoLockPool {
    pub(super) fn new() -> Self {
        let stripes = (0..CACHE_IO_LOCK_STRIPES).map(|_| Arc::new(AsyncRwLock::new(()))).collect();
        Self { stripes }
    }

    async fn read(&self, hash: &str) -> CacheIoReadGuard {
        let stripe = self.stripe(hash);
        CacheIoReadGuard { _guard: self.stripes[stripe].clone().read_owned().await }
    }

    async fn write(&self, hash: &str) -> CacheIoWriteGuards {
        self.write_hashes([hash]).await
    }

    async fn write_hashes<'a, I>(&self, hashes: I) -> CacheIoWriteGuards
    where
        I: IntoIterator<Item = &'a str>,
    {
        let stripes = hashes.into_iter().map(|hash| self.stripe(hash)).collect::<BTreeSet<_>>();
        let mut guards = Vec::with_capacity(stripes.len());
        for stripe in stripes {
            guards.push(self.stripes[stripe].clone().write_owned().await);
        }
        CacheIoWriteGuards { _guards: guards }
    }

    fn stripe(&self, hash: &str) -> usize {
        cache_io_lock_stripe_with_len(hash, self.stripes.len())
    }
}

#[cfg(test)]
pub(super) fn cache_io_lock_stripe(hash: &str) -> usize {
    cache_io_lock_stripe_with_len(hash, CACHE_IO_LOCK_STRIPES)
}

impl CacheZoneRuntime {
    pub(super) async fn io_read(&self, hash: &str) -> CacheIoReadGuard {
        self.io_locks.read(hash).await
    }

    pub(super) async fn io_write(&self, hash: &str) -> CacheIoWriteGuards {
        self.io_locks.write(hash).await
    }
}

fn cache_io_lock_stripe_with_len(hash: &str, stripe_len: usize) -> usize {
    let mut hasher = std::hash::DefaultHasher::new();
    hash.hash(&mut hasher);
    (hasher.finish() as usize) % stripe_len
}
