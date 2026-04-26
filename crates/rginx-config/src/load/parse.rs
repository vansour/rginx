use std::path::Path;

use rginx_core::{Error, Result};

use crate::model::Config;

pub(super) fn parse_config(contents: &str, source_path: &Path) -> Result<Config> {
    ron::de::from_str(contents).map_err(|error| {
        Error::Config(format!("failed to parse {}: {error}", source_path.display()))
    })
}
