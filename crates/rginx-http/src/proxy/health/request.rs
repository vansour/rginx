use super::*;

pub(in super::super) fn build_active_health_request(
    upstream: &Upstream,
    peer: &ResolvedUpstreamPeer,
    check: &ActiveHealthCheck,
) -> Result<Request<HttpBody>, Error> {
    let path = &check.path;
    let path: Uri = path.parse().map_err(|error| {
        Error::Server(format!(
            "failed to parse active health-check path `{path}` for peer `{}`: {error}",
            peer.display_url
        ))
    })?;
    let uri = build_proxy_uri(peer, &path, None).map_err(|error| {
        Error::Server(format!("failed to build active health-check uri: {error}"))
    })?;

    let mut builder = Request::builder().uri(uri).header(HOST, peer.upstream_authority.as_str());
    let request_protocol = if check.grpc_service.is_some() {
        if upstream.protocol == UpstreamProtocol::H2c {
            UpstreamProtocol::H2c
        } else {
            UpstreamProtocol::Http2
        }
    } else {
        upstream.protocol
    };

    let mut request = if let Some(service) = check.grpc_service.as_deref() {
        let body = encode_grpc_health_check_request(service);
        builder = builder
            .method(Method::POST)
            .version(upstream_request_version(request_protocol))
            .header(CONTENT_TYPE, HeaderValue::from_static("application/grpc"))
            .header(TE, HeaderValue::from_static("trailers"))
            .header(CONTENT_LENGTH, body.len().to_string());
        builder.body(full_body(body)).map_err(|error| {
            Error::Server(format!("failed to build active health-check request: {error}"))
        })
    } else {
        builder
            .method(Method::GET)
            .version(upstream_request_version(request_protocol))
            .body(full_body(Bytes::new()))
            .map_err(|error| {
                Error::Server(format!("failed to build active health-check request: {error}"))
            })
    }?;
    remove_redundant_host_header_for_authority_pseudo_header(
        request.headers_mut(),
        peer,
        request_protocol,
    );
    Ok(request)
}
