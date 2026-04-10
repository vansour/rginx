use super::super::grpc_web::{GrpcWebEncoding, GrpcWebMode};
use super::*;

pub(crate) fn detect_grpc_web_mode(
    headers: &HeaderMap,
) -> Result<Option<GrpcWebMode>, &'static str> {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return Ok(None);
    };
    let Ok(content_type_str) = content_type.to_str() else {
        return Ok(None);
    };

    let (mime, params) = split_content_type(content_type_str);
    let normalized_mime = mime.to_ascii_lowercase();
    if !normalized_mime.starts_with(GRPC_WEB_CONTENT_TYPE_PREFIX) {
        return Ok(None);
    }

    let (encoding, upstream_mime) =
        if normalized_mime.starts_with(GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX) {
            (
                GrpcWebEncoding::Text,
                normalized_mime.replacen(
                    GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX,
                    GRPC_CONTENT_TYPE_PREFIX,
                    1,
                ),
            )
        } else {
            (
                GrpcWebEncoding::Binary,
                normalized_mime.replacen(GRPC_WEB_CONTENT_TYPE_PREFIX, GRPC_CONTENT_TYPE_PREFIX, 1),
            )
        };
    let upstream_content_type =
        if params.is_empty() { upstream_mime } else { format!("{upstream_mime}; {params}") };

    let upstream_content_type = HeaderValue::from_str(&upstream_content_type)
        .map_err(|_| "invalid grpc-web content-type")?;

    Ok(Some(GrpcWebMode {
        downstream_content_type: content_type.clone(),
        upstream_content_type,
        encoding,
    }))
}

pub(super) fn grpc_protocol_request(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return false;
    };
    let Ok(content_type) = content_type.to_str() else {
        return false;
    };
    let (mime, _) = split_content_type(content_type);
    mime.to_ascii_lowercase().starts_with(GRPC_CONTENT_TYPE_PREFIX)
}

pub(crate) fn effective_upstream_request_timeout(
    headers: &HeaderMap,
    upstream_timeout: Duration,
) -> Result<Duration, String> {
    let grpc_timeout = parse_grpc_timeout(headers)?;
    Ok(grpc_timeout.map_or(upstream_timeout, |timeout| timeout.min(upstream_timeout)))
}

pub(crate) fn parse_grpc_timeout(headers: &HeaderMap) -> Result<Option<Duration>, String> {
    if !grpc_protocol_request(headers) {
        return Ok(None);
    }

    let Some(timeout) = headers.get(GRPC_TIMEOUT_HEADER) else {
        return Ok(None);
    };
    let value = timeout
        .to_str()
        .map_err(|_| format!("invalid {GRPC_TIMEOUT_HEADER} header: expected ASCII"))?;
    let value = value.trim();
    if value.len() < 2 {
        return Err(format!(
            "invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n"
        ));
    }

    let (amount, unit) = value.split_at(value.len() - 1);
    if amount.is_empty()
        || amount.len() > MAX_GRPC_TIMEOUT_DIGITS
        || !amount.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!(
            "invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n"
        ));
    }

    let amount = amount.parse::<u64>().map_err(|_| {
        format!("invalid {GRPC_TIMEOUT_HEADER} header: timeout value is out of range")
    })?;
    Ok(Some(grpc_timeout_duration(amount, unit).ok_or_else(|| {
        format!(
            "invalid {GRPC_TIMEOUT_HEADER} header: expected 1-8 ASCII digits followed by H/M/S/m/u/n"
        )
    })?))
}

fn grpc_timeout_duration(amount: u64, unit: &str) -> Option<Duration> {
    match unit {
        "H" => amount.checked_mul(60 * 60).map(Duration::from_secs),
        "M" => amount.checked_mul(60).map(Duration::from_secs),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}
