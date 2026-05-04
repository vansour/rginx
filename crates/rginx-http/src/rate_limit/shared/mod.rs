use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use rginx_core::RouteRateLimit;

use crate::cache::shared::memory::{SharedMemorySegment, SharedMemorySegmentConfig};

mod bucket;
mod document;
mod lock;

use bucket::{SharedTokenBucket, maybe_cleanup_document};
use document::{
    SharedRateLimitDocument, bucket_key, read_document, shm_capacity_bytes, stable_hash,
    unix_time_ms, write_document,
};
use lock::FileLock;

pub(super) struct SharedRateLimitStore {
    segment_config: SharedMemorySegmentConfig,
    lock_path: PathBuf,
    lock_contention_total: AtomicU64,
}

impl SharedRateLimitStore {
    pub(super) fn new(config_path: &Path) -> io::Result<Self> {
        let identity = format!("rate-limit:{}", config_path.display());
        Self::from_identity(&identity)
    }

    #[cfg(test)]
    pub(super) fn for_identity(identity: &str) -> io::Result<Self> {
        Self::from_identity(identity)
    }

    fn from_identity(identity: &str) -> io::Result<Self> {
        let identity_hash = stable_hash(identity);
        let lock_path =
            std::env::temp_dir().join(format!("rginx-rate-limit-{identity_hash:016x}.lock"));
        Ok(Self {
            segment_config: SharedMemorySegmentConfig::for_identity(identity, shm_capacity_bytes()),
            lock_path,
            lock_contention_total: AtomicU64::new(0),
        })
    }

    pub(super) fn check(
        &self,
        route: &str,
        client_ip: IpAddr,
        policy: RouteRateLimit,
    ) -> io::Result<bool> {
        self.check_at(route, client_ip, policy, unix_time_ms(std::time::SystemTime::now()))
    }

    pub(super) fn check_at(
        &self,
        route: &str,
        client_ip: IpAddr,
        policy: RouteRateLimit,
        now_unix_ms: u64,
    ) -> io::Result<bool> {
        let _lock = self.lock()?;
        let segment = self.open_or_create_segment()?;
        let mut document = read_document(&segment)?;
        maybe_cleanup_document(&mut document, now_unix_ms);

        let key = bucket_key(route, client_ip);
        let bucket = document
            .buckets
            .entry(key)
            .or_insert_with(|| SharedTokenBucket::new(policy, now_unix_ms));
        let allowed = bucket.try_acquire(policy, now_unix_ms);
        write_document(&segment, &document)?;
        Ok(allowed)
    }

    #[cfg(test)]
    pub(super) fn unlink_for_test(&self) -> io::Result<()> {
        let _ = std::fs::remove_file(&self.lock_path);
        SharedMemorySegment::unlink(&self.segment_config)
    }

    fn lock(&self) -> io::Result<FileLock> {
        lock::acquire(&self.lock_path, &self.lock_contention_total)
    }

    fn open_or_create_segment(&self) -> io::Result<SharedMemorySegment> {
        match SharedMemorySegment::attach(&self.segment_config) {
            Ok(segment) => Ok(segment),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                write_document(&segment, &SharedRateLimitDocument::default())?;
                Ok(segment)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let _ = SharedMemorySegment::unlink(&self.segment_config);
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                write_document(&segment, &SharedRateLimitDocument::default())?;
                Ok(segment)
            }
            Err(error) => Err(error),
        }
    }
}
