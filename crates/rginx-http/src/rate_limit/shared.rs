use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io;
use std::net::IpAddr;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rginx_core::RouteRateLimit;
use serde::{Deserialize, Serialize};

use crate::cache::shared::memory::{SharedMemorySegment, SharedMemorySegmentConfig};

const DOCUMENT_LEN_BYTES: usize = 8;
const DOCUMENT_VERSION: u32 = 1;
const DEFAULT_SHM_CAPACITY_BYTES: usize = 4 * 1024 * 1024;
const CLEANUP_INTERVAL_MS: u64 = 30_000;

pub(super) struct SharedRateLimitStore {
    segment_config: SharedMemorySegmentConfig,
    lock_path: PathBuf,
    lock_contention_total: AtomicU64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SharedRateLimitDocument {
    version: u32,
    #[serde(default)]
    next_cleanup_unix_ms: u64,
    #[serde(default)]
    buckets: BTreeMap<String, SharedTokenBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedTokenBucket {
    requests_per_sec: u32,
    burst: u32,
    tokens: f64,
    last_refill_unix_ms: u64,
    last_seen_unix_ms: u64,
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
        self.check_at(route, client_ip, policy, unix_time_ms(SystemTime::now()))
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
        let mut document = self.read_document(&segment)?;
        maybe_cleanup_document(&mut document, now_unix_ms);

        let bucket_key = bucket_key(route, client_ip);
        let bucket = document
            .buckets
            .entry(bucket_key)
            .or_insert_with(|| SharedTokenBucket::new(policy, now_unix_ms));
        let allowed = bucket.try_acquire(policy, now_unix_ms);
        self.write_document(&segment, &document)?;
        Ok(allowed)
    }

    #[cfg(test)]
    pub(super) fn unlink_for_test(&self) -> io::Result<()> {
        let _ = std::fs::remove_file(&self.lock_path);
        SharedMemorySegment::unlink(&self.segment_config)
    }

    fn lock(&self) -> io::Result<FileLock> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(FileLock { file });
        }

        let error = io::Error::last_os_error();
        let would_block = error.kind() == io::ErrorKind::WouldBlock
            || error.raw_os_error().is_some_and(|code| code == libc::EWOULDBLOCK);
        if !would_block {
            return Err(error);
        }

        self.lock_contention_total.fetch_add(1, Ordering::Relaxed);
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(FileLock { file });
        }
        Err(io::Error::last_os_error())
    }

    fn open_or_create_segment(&self) -> io::Result<SharedMemorySegment> {
        match SharedMemorySegment::attach(&self.segment_config) {
            Ok(segment) => Ok(segment),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                self.write_document(&segment, &SharedRateLimitDocument::default())?;
                Ok(segment)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let _ = SharedMemorySegment::unlink(&self.segment_config);
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                self.write_document(&segment, &SharedRateLimitDocument::default())?;
                Ok(segment)
            }
            Err(error) => Err(error),
        }
    }

    fn read_document(&self, segment: &SharedMemorySegment) -> io::Result<SharedRateLimitDocument> {
        let payload_capacity = segment.payload_capacity();
        if payload_capacity < DOCUMENT_LEN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared rate-limit payload is too small for document length",
            ));
        }
        let len_bytes = segment.read_payload(0, DOCUMENT_LEN_BYTES)?;
        let len = u64::from_le_bytes(
            len_bytes
                .as_slice()
                .try_into()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        if len == 0 {
            return Ok(SharedRateLimitDocument::default());
        }
        let len = usize::try_from(len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared rate-limit document length is too large",
            )
        })?;
        let document_capacity = payload_capacity.saturating_sub(DOCUMENT_LEN_BYTES);
        if len > document_capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "shared rate-limit document length exceeds payload capacity: length {len}, capacity {document_capacity}"
                ),
            ));
        }
        let bytes = segment.read_payload(DOCUMENT_LEN_BYTES, len)?;
        let document: SharedRateLimitDocument =
            serde_json::from_slice(&bytes).map_err(invalid_data_error)?;
        if document.version != DOCUMENT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported shared rate-limit document version `{}`", document.version),
            ));
        }
        Ok(document)
    }

    fn write_document(
        &self,
        segment: &SharedMemorySegment,
        document: &SharedRateLimitDocument,
    ) -> io::Result<()> {
        let bytes = serde_json::to_vec(document).map_err(invalid_data_error)?;
        let document_capacity = segment.payload_capacity().saturating_sub(DOCUMENT_LEN_BYTES);
        if bytes.len() > document_capacity {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!(
                    "shared rate-limit document exceeds capacity: length {}, capacity {}",
                    bytes.len(),
                    document_capacity
                ),
            ));
        }
        let len = u64::try_from(bytes.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared rate-limit document length is too large",
            )
        })?;
        segment.write_payload(0, &len.to_le_bytes())?;
        segment.write_payload(DOCUMENT_LEN_BYTES, &bytes)?;
        Ok(())
    }
}

impl Default for SharedRateLimitDocument {
    fn default() -> Self {
        Self { version: DOCUMENT_VERSION, next_cleanup_unix_ms: 0, buckets: BTreeMap::new() }
    }
}

impl SharedTokenBucket {
    fn new(policy: RouteRateLimit, now_unix_ms: u64) -> Self {
        Self {
            requests_per_sec: policy.requests_per_sec,
            burst: policy.burst,
            tokens: bucket_capacity(policy),
            last_refill_unix_ms: now_unix_ms,
            last_seen_unix_ms: now_unix_ms,
        }
    }

    fn try_acquire(&mut self, policy: RouteRateLimit, now_unix_ms: u64) -> bool {
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

fn maybe_cleanup_document(document: &mut SharedRateLimitDocument, now_unix_ms: u64) {
    if now_unix_ms < document.next_cleanup_unix_ms {
        return;
    }

    document.buckets.retain(|_, bucket| !bucket.is_evictable(now_unix_ms));
    document.next_cleanup_unix_ms = now_unix_ms.saturating_add(CLEANUP_INTERVAL_MS);
}

fn shm_capacity_bytes() -> usize {
    std::env::var_os("RGINX_RATE_LIMIT_SHM_CAPACITY_BYTES")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|capacity| *capacity >= 4 * 1024)
        .unwrap_or(DEFAULT_SHM_CAPACITY_BYTES)
}

fn bucket_key(route: &str, client_ip: IpAddr) -> String {
    format!("{route}\0{client_ip}")
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn bucket_capacity(policy: RouteRateLimit) -> f64 {
    (policy.burst + 1) as f64
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn invalid_data_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

struct FileLock {
    file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}
