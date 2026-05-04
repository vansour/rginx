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

        if normalized == "." || normalized.ends_with('.') {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` must not end with a bare `.`"
            )));
        }

        if has_unsupported_wildcard(&normalized) {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` uses unsupported wildcard syntax; supported forms are exact names, `.example.com`, leading `*.` wildcards, and trailing `.*` wildcards"
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

fn has_unsupported_wildcard(name: &str) -> bool {
    if !name.contains('*') {
        return false;
    }

    if let Some(suffix) = name.strip_prefix("*.") {
        return suffix.is_empty() || suffix.contains('*');
    }

    if let Some(prefix) = name.strip_suffix(".*") {
        return prefix.is_empty() || prefix.contains('*');
    }

    true
}
