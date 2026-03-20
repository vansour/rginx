use std::sync::Arc;

use crate::ConfigSnapshot;

pub type SharedConfig = Arc<ConfigSnapshot>;
