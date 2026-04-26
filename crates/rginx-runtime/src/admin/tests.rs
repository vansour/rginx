use std::path::{Path, PathBuf};

use super::{INSTALLED_ADMIN_SOCKET_PATH, admin_socket_path_for_config};

#[test]
fn installed_config_uses_run_admin_socket() {
    assert_eq!(
        admin_socket_path_for_config(Path::new("/etc/rginx/rginx.ron")),
        PathBuf::from(INSTALLED_ADMIN_SOCKET_PATH)
    );
}

#[test]
fn custom_config_uses_neighbor_admin_socket() {
    assert_eq!(
        admin_socket_path_for_config(Path::new("/tmp/site.ron")),
        PathBuf::from("/tmp/site.admin.sock")
    );
}
