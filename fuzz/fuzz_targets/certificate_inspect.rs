#![no_main]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static CERT_PATH: OnceLock<PathBuf> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let path = certificate_path();
    let _ = std::fs::write(path, data);
    rginx_http::inspect_certificate_for_fuzzing(path);
});

fn certificate_path() -> &'static Path {
    CERT_PATH
        .get_or_init(|| {
            let root = std::env::temp_dir().join("rginx-fuzz-certificate-inspect");
            let _ = std::fs::create_dir_all(&root);
            root.join("bundle.pem")
        })
        .as_path()
}
