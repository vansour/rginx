use std::fs;
use std::path::Path;

use rginx_core::Result;

use crate::model::Config;

mod env_expand;
mod include;
mod parse;
mod preprocess;
#[cfg(test)]
mod tests;

#[cfg(test)]
use env_expand::expand_env_placeholders_in_ron_strings;

pub fn load_from_path(path: impl AsRef<Path>) -> Result<Config> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)?;
    load_from_str(&contents, path)
}

pub fn load_from_str(contents: &str, source_path: impl AsRef<Path>) -> Result<Config> {
    let source_path = source_path.as_ref();
    let expanded = preprocess::preprocess_source(contents, source_path)?;
    parse::parse_config(&expanded, source_path)
}
