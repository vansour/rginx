use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) fn process_private_temp_file(
    slot: &'static OnceLock<PathBuf>,
    root_name: &str,
    file_name: &str,
) -> &'static Path {
    slot.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("{root_name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("fuzz temp directory should be created");
        root.join(file_name)
    })
    .as_path()
}

pub(crate) fn write_input(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}
