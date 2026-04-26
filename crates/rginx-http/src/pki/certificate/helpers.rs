use rasn::types::{Integer, IntegerType};
use sha2::{Digest, Sha256};

use rasn_pkix::Time;

pub(super) fn bytes_to_lossy_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub(super) fn codepoints_to_string(values: impl IntoIterator<Item = u32>) -> String {
    values.into_iter().map(|value| char::from_u32(value).unwrap_or('\u{fffd}')).collect()
}

pub(super) fn fingerprint_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_string(digest.as_slice())
}

pub(super) fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

pub(super) fn integer_to_serial_string(value: &Integer) -> String {
    let (bytes, len) = value.to_signed_bytes_be();
    let bytes = &bytes.as_ref()[..len];
    if bytes.is_empty() {
        return "00".to_string();
    }
    if value.is_negative() {
        return format!("{value}");
    }
    hex_string(bytes)
}

pub(super) fn integer_to_u32(value: &Integer) -> Option<u32> {
    let (bytes, len) = value.to_signed_bytes_be();
    let bytes = &bytes.as_ref()[..len];
    if value.is_negative() {
        return None;
    }
    if bytes.is_empty() {
        return Some(0);
    }
    if bytes.len() > 4 {
        return None;
    }
    let mut padded = [0u8; 4];
    let start = padded.len().saturating_sub(bytes.len());
    padded[start..].copy_from_slice(bytes);
    Some(u32::from_be_bytes(padded))
}

pub(super) fn time_to_unix_ms(time: Time) -> Option<u64> {
    let millis = match time {
        Time::Utc(value) => value.timestamp_millis(),
        Time::General(value) => value.timestamp_millis(),
    };
    u64::try_from(millis).ok()
}

pub(super) fn time_to_unix_secs(time: Time) -> Option<i64> {
    Some(match time {
        Time::Utc(value) => value.timestamp(),
        Time::General(value) => value.timestamp(),
    })
}
