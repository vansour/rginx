#![no_main]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static CERT_PATH: OnceLock<PathBuf> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let path = responder_cert_path();
    let _ = std::fs::write(path, data);
    rginx_http::discover_ocsp_responder_urls_for_fuzzing(path);
});

fn responder_cert_path() -> &'static Path {
    CERT_PATH
        .get_or_init(|| {
            let root = std::env::temp_dir().join("rginx-fuzz-ocsp-responder-discovery");
            let _ = std::fs::create_dir_all(&root);
            root.join("server.pem")
        })
        .as_path()
}
