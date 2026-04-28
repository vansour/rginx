use super::*;

mod background;
mod cache_lookup;
mod logging;
mod primary;

use background::spawn_background_cache_refresh;
use cache_lookup::resolve_forward_cache;
use logging::log_successful_attempt;

pub use primary::forward_request;
