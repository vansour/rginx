use std::collections::BTreeMap;
use std::io;
use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cache::shared::memory::SharedMemorySegment;

use super::bucket::SharedTokenBucket;

const DOCUMENT_LEN_BYTES: usize = 8;
const DOCUMENT_VERSION: u32 = 1;
const DEFAULT_SHM_CAPACITY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SharedRateLimitDocument {
    pub(super) version: u32,
    #[serde(default)]
    pub(super) next_cleanup_unix_ms: u64,
    #[serde(default)]
    pub(super) buckets: BTreeMap<String, SharedTokenBucket>,
}

impl Default for SharedRateLimitDocument {
    fn default() -> Self {
        Self { version: DOCUMENT_VERSION, next_cleanup_unix_ms: 0, buckets: BTreeMap::new() }
    }
}

pub(super) fn read_document(segment: &SharedMemorySegment) -> io::Result<SharedRateLimitDocument> {
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
        io::Error::new(io::ErrorKind::InvalidData, "shared rate-limit document length is too large")
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

pub(super) fn write_document(
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
        io::Error::new(io::ErrorKind::InvalidData, "shared rate-limit document length is too large")
    })?;
    segment.write_payload(0, &len.to_le_bytes())?;
    segment.write_payload(DOCUMENT_LEN_BYTES, &bytes)?;
    Ok(())
}

pub(super) fn shm_capacity_bytes() -> usize {
    std::env::var_os("RGINX_RATE_LIMIT_SHM_CAPACITY_BYTES")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|capacity| *capacity >= 4 * 1024)
        .unwrap_or(DEFAULT_SHM_CAPACITY_BYTES)
}

pub(super) fn bucket_key(route: &str, client_ip: IpAddr) -> String {
    format!("{route}\0{client_ip}")
}

pub(super) fn stable_hash(value: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    value.bytes().fold(FNV_OFFSET, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME))
}

pub(super) fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub(super) fn invalid_data_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
