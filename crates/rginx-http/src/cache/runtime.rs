use super::*;

mod context;
mod fill_lock;
mod support;
mod zone;

pub(in crate::cache) use support::PurgeSelector;
pub(in crate::cache) use support::{
    CacheEntryLifecyclePhase, build_conditional_headers, lifecycle_phase,
    remove_cache_entry_if_matches, remove_cache_files_if_unreferenced, remove_cache_files_locked,
};

pub(crate) fn with_cache_status(mut response: HttpResponse, status: CacheStatus) -> HttpResponse {
    response.headers_mut().insert(CACHE_STATUS_HEADER, status.as_header_value());
    response
}
