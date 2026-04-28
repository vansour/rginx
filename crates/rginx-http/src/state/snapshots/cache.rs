use serde::{Deserialize, Serialize};

use crate::cache::CacheZoneRuntimeSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStatsSnapshot {
    pub zones: Vec<CacheZoneRuntimeSnapshot>,
}
