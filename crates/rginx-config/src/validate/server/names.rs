use std::collections::HashSet;

use rginx_core::{Error, Result};

/// Validates server-name ownership rules across the default server and vhosts.
pub(super) fn validate_server_names(
    owner_label: &str,
    server_names: &[String],
    all_server_names: &mut HashSet<String>,
) -> Result<()> {
    for name in server_names {
        let normalized = name.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(Error::Config(format!("{owner_label} server_name must not be empty")));
        }

        if normalized.contains('*')
            && (!normalized.starts_with("*.")
                || normalized[2..].is_empty()
                || normalized[2..].contains('*'))
        {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` uses unsupported wildcard syntax; only leading `*.` patterns are supported"
            )));
        }

        if normalized.contains('/') {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` should not contain path separator"
            )));
        }

        if !all_server_names.insert(normalized) {
            return Err(Error::Config(format!(
                "duplicate server_name `{name}` across server and servers"
            )));
        }
    }

    Ok(())
}
