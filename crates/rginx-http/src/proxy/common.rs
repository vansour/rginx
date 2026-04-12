use super::*;

pub(super) fn build_proxy_uri(
    peer: &UpstreamPeer,
    original_uri: &Uri,
    strip_prefix: Option<&str>,
) -> Result<Uri, http::Error> {
    let original_path = original_uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");

    let path_and_query = if let Some(prefix) = strip_prefix {
        if let Some(stripped) = original_path.strip_prefix(prefix) {
            if stripped.is_empty() || stripped.starts_with('?') {
                if stripped.is_empty() { "/" } else { stripped }
            } else if stripped.starts_with('/') {
                stripped
            } else {
                original_path
            }
        } else {
            original_path
        }
    } else {
        original_path
    };

    Uri::builder()
        .scheme(peer.scheme.as_str())
        .authority(peer.authority.as_str())
        .path_and_query(path_and_query)
        .build()
}

pub(super) fn split_content_type(content_type: &str) -> (&str, &str) {
    let mut parts = content_type.splitn(2, ';');
    let mime = parts.next().unwrap_or_default().trim();
    let params = parts.next().unwrap_or_default().trim();
    (mime, params)
}

pub(super) fn append_header_map(destination: &mut HeaderMap, source: &HeaderMap) {
    for name in source.keys() {
        for value in source.get_all(name).iter() {
            destination.append(name.clone(), value.clone());
        }
    }
}

pub(super) fn sanitize_request_headers(
    headers: &mut HeaderMap,
    authority: &str,
    original_host: Option<HeaderValue>,
    client_address: &ClientAddress,
    forwarded_proto: &str,
    preserve_host: bool,
    proxy_set_headers: &[(HeaderName, HeaderValue)],
    grpc_web_mode: Option<&GrpcWebMode>,
) -> Result<(), http::header::InvalidHeaderValue> {
    let upgrade_protocol = extract_upgrade_protocol(headers);
    let te_trailers = preserved_te_trailers_value(headers);
    remove_hop_by_hop_headers(headers, upgrade_protocol.is_some());

    if preserve_host {
        if let Some(ref host) = original_host {
            headers.insert(HOST, host.clone());
        } else {
            headers.insert(HOST, HeaderValue::from_str(authority)?);
        }
    } else {
        headers.insert(HOST, HeaderValue::from_str(authority)?);
    }

    headers.insert("x-forwarded-proto", HeaderValue::from_str(forwarded_proto)?);

    if let Some(host) = original_host {
        headers.insert("x-forwarded-host", host);
    }

    headers.insert("x-forwarded-for", HeaderValue::from_str(&client_address.forwarded_for)?);

    for (name, value) in proxy_set_headers {
        headers.insert(name.clone(), value.clone());
    }

    if let Some(grpc_web_mode) = grpc_web_mode {
        headers.insert(CONTENT_TYPE, grpc_web_mode.upstream_content_type.clone());
        headers.remove(CONTENT_LENGTH);
        headers.remove("x-grpc-web");
        if headers.get(TE).is_none() {
            headers.insert(TE, HeaderValue::from_static("trailers"));
        }
    } else if headers.get(TE).is_none()
        && let Some(te_trailers) = te_trailers
    {
        headers.insert(TE, te_trailers);
    }

    if let Some(upgrade_protocol) = upgrade_protocol {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, upgrade_protocol);
    }

    Ok(())
}

pub(super) fn preserved_te_trailers_value(headers: &HeaderMap) -> Option<HeaderValue> {
    let mut saw_te = false;

    for value in headers.get_all(TE) {
        let value = value.to_str().ok()?;

        for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            saw_te = true;
            if !token.eq_ignore_ascii_case("trailers") {
                return None;
            }
        }
    }

    saw_te.then(|| HeaderValue::from_static("trailers"))
}

pub(super) fn sanitize_response_headers(headers: &mut HeaderMap, preserve_upgrade: bool) {
    let upgrade_protocol = if preserve_upgrade { headers.get(UPGRADE).cloned() } else { None };
    remove_hop_by_hop_headers(headers, preserve_upgrade);

    if let Some(upgrade_protocol) = upgrade_protocol {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, upgrade_protocol);
    }
}

pub(super) fn remove_hop_by_hop_headers(headers: &mut HeaderMap, preserve_upgrade: bool) {
    let mut extra_headers = Vec::new();

    for value in headers.get_all(CONNECTION) {
        if let Ok(value) = value.to_str() {
            for item in value.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(name) = HeaderName::from_bytes(trimmed.as_bytes()) {
                    extra_headers.push(name);
                }
            }
        }
    }

    for name in extra_headers {
        if preserve_upgrade && name == UPGRADE {
            continue;
        }
        headers.remove(name);
    }

    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        if preserve_upgrade && (name == CONNECTION || name == UPGRADE) {
            continue;
        }
        headers.remove(name);
    }

    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

pub(super) fn is_upgrade_request(version: Version, headers: &HeaderMap) -> bool {
    version == Version::HTTP_11 && extract_upgrade_protocol(headers).is_some()
}

pub(super) fn is_upgrade_response(status: StatusCode, headers: &HeaderMap) -> bool {
    status == StatusCode::SWITCHING_PROTOCOLS && headers.contains_key(UPGRADE)
}

pub(super) fn extract_upgrade_protocol(headers: &HeaderMap) -> Option<HeaderValue> {
    connection_header_contains_token(headers, "upgrade").then(|| headers.get(UPGRADE).cloned())?
}

pub(super) fn connection_header_contains_token(headers: &HeaderMap, token: &str) -> bool {
    headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|item| item.trim().eq_ignore_ascii_case(token))
        })
    })
}

pub(super) fn upstream_request_version(protocol: UpstreamProtocol) -> Version {
    match protocol {
        UpstreamProtocol::Http3 => Version::HTTP_3,
        UpstreamProtocol::Http2 => Version::HTTP_2,
        UpstreamProtocol::Auto | UpstreamProtocol::Http1 => Version::HTTP_11,
    }
}
