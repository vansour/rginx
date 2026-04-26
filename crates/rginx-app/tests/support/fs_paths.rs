use super::*;

pub(super) fn read_optional_log(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(contents) if contents.is_empty() => "<empty>".to_string(),
        Ok(contents) => contents,
        Err(error) => format!("<unavailable: {error}>"),
    }
}

pub(super) fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

pub(super) fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}
