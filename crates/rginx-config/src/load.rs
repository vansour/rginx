use std::fs;
use std::path::Path;

use rginx_core::{Error, Result};

use crate::model::Config;

pub fn load_from_path(path: impl AsRef<Path>) -> Result<Config> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)?;
    ron::de::from_str(&contents)
        .map_err(|error| Error::Config(format!("failed to parse {}: {error}", path.display())))
}
