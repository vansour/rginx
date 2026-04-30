use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) fn tls_identity_is_managed(
    tls: &crate::model::ServerTlsConfig,
    base_dir: &Path,
    managed_identity_pairs: &HashSet<(PathBuf, PathBuf)>,
) -> bool {
    managed_identity_pairs.contains(&(
        super::super::resolve_path(base_dir, tls.cert_path.clone()),
        super::super::resolve_path(base_dir, tls.key_path.clone()),
    ))
}
