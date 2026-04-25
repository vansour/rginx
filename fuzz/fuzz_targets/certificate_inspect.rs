#![no_main]

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static CERT_PATH: OnceLock<PathBuf> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let path = certificate_path();
    if common::write_input(path, data).is_err() {
        return;
    }
    rginx_http::inspect_certificate_for_fuzzing(path);
});

fn certificate_path() -> &'static Path {
    common::process_private_temp_file(&CERT_PATH, "rginx-fuzz-certificate-inspect", "bundle.pem")
}
