use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::HeaderValue;
use tracing::warn;

static HTTP_DATE_CACHE: OnceLock<Mutex<CachedHttpDate>> = OnceLock::new();

struct CachedHttpDate {
    unix_epoch_seconds: u64,
    value: HeaderValue,
}

pub(super) fn current_http_date() -> HeaderValue {
    let unix_epoch_seconds = current_unix_epoch_seconds();
    let cache = HTTP_DATE_CACHE.get_or_init(|| Mutex::new(CachedHttpDate::new(unix_epoch_seconds)));
    let mut cached = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if cached.unix_epoch_seconds != unix_epoch_seconds {
        *cached = CachedHttpDate::new(unix_epoch_seconds);
    }
    cached.value.clone()
}

impl CachedHttpDate {
    fn new(unix_epoch_seconds: u64) -> Self {
        let timestamp = UNIX_EPOCH + Duration::from_secs(unix_epoch_seconds);
        let value = httpdate::fmt_http_date(timestamp);
        let value =
            HeaderValue::from_str(&value).expect("formatted HTTP date should be a valid header");
        Self { unix_epoch_seconds, value }
    }
}

fn current_unix_epoch_seconds() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(error) => {
            warn!(%error, "system clock predates UNIX_EPOCH while computing Date header");
            0
        }
    }
}
