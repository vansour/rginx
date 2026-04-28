use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::super::vary::canonical_vary_headers;
use super::*;

pub(in crate::cache) fn cache_key_hash(key: &str) -> String {
    cache_bytes_hash(key.as_bytes())
}

pub(in crate::cache) fn cache_variant_key(
    base_key: &str,
    vary: &[CachedVaryHeaderValue],
) -> String {
    if vary.is_empty() {
        return base_key.to_string();
    }

    let mut signature = Vec::new();
    for header in canonical_vary_headers(vary) {
        signature.extend_from_slice(header.name.as_str().as_bytes());
        signature.push(0);
        if let Some(value) = &header.value {
            signature.extend_from_slice(value.as_bytes());
        }
        signature.push(0xff);
    }
    format!("{base_key}|vary:{}", cache_bytes_hash(&signature))
}

pub(in crate::cache) fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn cache_bytes_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
