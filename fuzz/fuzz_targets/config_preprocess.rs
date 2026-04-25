#![no_main]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static CONFIG_SOURCE_PATH: OnceLock<PathBuf> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let _ = rginx_config::load::load_from_str(&source, config_source_path());
});

fn config_source_path() -> &'static Path {
    CONFIG_SOURCE_PATH
        .get_or_init(|| {
            let root = std::env::temp_dir().join("rginx-fuzz-config-root");
            let _ = std::fs::create_dir_all(&root);
            root.join("inline.ron")
        })
        .as_path()
}
