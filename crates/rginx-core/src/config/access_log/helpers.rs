pub(super) fn is_access_log_variable_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(super) fn fallback_access_log_value(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

pub(super) fn fallback_access_log_option(value: Option<&str>) -> &str {
    value.filter(|value| !value.is_empty()).unwrap_or("-")
}
