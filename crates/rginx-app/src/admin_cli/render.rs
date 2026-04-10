use super::*;

pub(super) fn render_last_reload(result: Option<&rginx_http::ReloadResultSnapshot>) -> String {
    let Some(result) = result else {
        return "-".to_string();
    };

    let finished_at = result
        .finished_at_unix_ms
        .checked_div(1000)
        .and_then(|seconds| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds)))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|| result.finished_at_unix_ms.to_string());

    match &result.outcome {
        rginx_http::ReloadOutcomeSnapshot::Success { revision } => {
            format!(
                "success revision={revision} active_revision={} finished_at_unix_s={finished_at}",
                result.active_revision
            )
        }
        rginx_http::ReloadOutcomeSnapshot::Failure { error } => {
            format!(
                "failure active_revision={} rollback_preserved_revision={} error={error:?} finished_at_unix_s={finished_at}",
                result.active_revision,
                result
                    .rollback_preserved_revision
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            )
        }
    }
}

pub(super) fn render_reload_active_revision(
    result: Option<&rginx_http::ReloadResultSnapshot>,
) -> String {
    result.map(|result| result.active_revision.to_string()).unwrap_or_else(|| "-".to_string())
}

pub(super) fn render_reload_rollback_revision(
    result: Option<&rginx_http::ReloadResultSnapshot>,
) -> String {
    result
        .and_then(|result| result.rollback_preserved_revision)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn render_reload_tls_certificate_changes(
    result: Option<&rginx_http::ReloadResultSnapshot>,
) -> String {
    let Some(result) = result else {
        return "-".to_string();
    };
    if result.tls_certificate_changes.is_empty() {
        return "-".to_string();
    }
    result.tls_certificate_changes.join(",")
}

pub(super) fn render_optional_string_list(values: Option<&[String]>) -> String {
    match values {
        Some(values) if !values.is_empty() => values.join(","),
        _ => "-".to_string(),
    }
}

pub(super) fn print_record<const N: usize>(kind: &str, fields: [(&str, String); N]) {
    let mut rendered = String::from("kind=");
    rendered.push_str(kind);
    for (key, value) in fields {
        rendered.push(' ');
        rendered.push_str(key);
        rendered.push('=');
        rendered.push_str(&encode_record_value(&value));
    }
    println!("{rendered}");
}

fn encode_record_value(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '.' | ':' | '/' | '-' | '_' | ',' | '*' | '[' | ']' | '|')
        })
    {
        value.to_string()
    } else {
        serde_json::to_string(value).expect("record value should encode as JSON string")
    }
}
