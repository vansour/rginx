pub mod compile;
pub mod load;
pub mod model;
pub mod validate;

use std::path::Path;

use rginx_core::{ConfigSnapshot, Result};

pub fn load_and_compile(path: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    let path = path.as_ref();
    let raw = load::load_from_path(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    compile::compile_with_base(raw, base_dir)
}

pub fn load_and_compile_from_str(
    contents: &str,
    config_path: impl AsRef<Path>,
) -> Result<ConfigSnapshot> {
    let config_path = config_path.as_ref();
    let raw = load::load_from_str(contents, config_path)?;
    let base_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    compile::compile_with_base(raw, base_dir)
}
