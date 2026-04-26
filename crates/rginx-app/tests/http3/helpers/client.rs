use super::*;

pub(crate) struct Http3Response {
    pub(crate) status: StatusCode,
    pub(crate) headers: std::collections::HashMap<String, String>,
    pub(crate) body: Vec<u8>,
}

pub(crate) async fn http3_get(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, "GET", path, &[], None, None, cert_pem).await
}

pub(crate) async fn http3_get_with_client_identity(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, "GET", path, &[], None, client_identity, cert_pem)
        .await
}

pub(crate) async fn http3_request(
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, method, path, headers, body, None, cert_pem).await
}

pub(crate) async fn http3_streaming_get_two_chunks(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
) -> Result<(StatusCode, Bytes, Bytes), String> {
    let endpoint = http3_client_endpoint(None, cert_pem, false)?;
    let connection = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?
        .await
        .map_err(|error| format!("quic connect failed: {error}"))?;
    let connection_handle = connection.clone();

    let (mut driver, mut send_request) =
        client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(|error| format!("failed to initialize http3 client: {error}"))?;
    let mut driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let result = async {
        let request = Request::builder()
            .method("GET")
            .uri(format!("https://{server_name}:{}{path}", listen_addr.port()))
            .body(())
            .expect("http3 request should build");
        let mut request_stream = send_request
            .send_request(request)
            .await
            .map_err(|error| format!("failed to send http3 request: {error}"))?;
        request_stream
            .finish()
            .await
            .map_err(|error| format!("failed to finish http3 request: {error}"))?;

        let response = request_stream
            .recv_response()
            .await
            .map_err(|error| format!("failed to receive http3 response headers: {error}"))?;
        let status = response.status();
        let mut first =
            tokio::time::timeout(Duration::from_millis(500), request_stream.recv_data())
                .await
                .map_err(|_| "timed out waiting for first http3 response chunk".to_string())?
                .map_err(|error| format!("failed to receive first http3 response chunk: {error}"))?
                .ok_or_else(|| {
                    "http3 response body ended before the first chunk arrived".to_string()
                })?;
        let mut second = tokio::time::timeout(Duration::from_secs(2), request_stream.recv_data())
            .await
            .map_err(|_| "timed out waiting for second http3 response chunk".to_string())?
            .map_err(|error| format!("failed to receive second http3 response chunk: {error}"))?
            .ok_or_else(|| {
                "http3 response body ended before the second chunk arrived".to_string()
            })?;

        while request_stream
            .recv_data()
            .await
            .map_err(|error| format!("failed to drain remaining http3 response body: {error}"))?
            .is_some()
        {}
        let _ = request_stream
            .recv_trailers()
            .await
            .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

        Ok((
            status,
            first.copy_to_bytes(first.remaining()),
            second.copy_to_bytes(second.remaining()),
        ))
    }
    .await;

    connection_handle.close(quinn::VarInt::from_u32(0), b"done");
    endpoint.close(quinn::VarInt::from_u32(0), b"done");
    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }

    result
}

pub(crate) async fn http3_request_inner(
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    let endpoint = http3_client_endpoint(client_identity, cert_pem, false)?;
    let (response, _) = http3_request_with_endpoint(
        &endpoint,
        listen_addr,
        server_name,
        method,
        path,
        headers,
        body,
        false,
        Duration::ZERO,
    )
    .await?;
    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    Ok(response)
}

pub(crate) fn http3_client_endpoint(
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
    enable_early_data: bool,
) -> Result<quinn::Endpoint, String> {
    let roots = root_store_from_pem(cert_pem)?;
    let client_crypto = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|error| format!("failed to constrain TLS versions for http3 client: {error}"))?
    .with_root_certificates(roots);
    let mut client_crypto = match client_identity {
        Some((cert_path, key_path)) => {
            let certs = load_certs_from_path(cert_path)?;
            let key = load_private_key_from_path(key_path)?;
            client_crypto
                .with_client_auth_cert(certs, key)
                .map_err(|error| format!("failed to configure HTTP/3 client cert: {error}"))?
        }
        None => client_crypto.with_no_client_auth(),
    };
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    client_crypto.enable_early_data = enable_early_data;

    let client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client_crypto)
            .map_err(|error| format!("failed to build quic client config: {error}"))?,
    ));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn http3_request_with_endpoint(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    use_0rtt: bool,
    linger_after_response: Duration,
) -> Result<(Http3Response, Option<bool>), String> {
    let connecting = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?;
    let (connection, zero_rtt_accepted) = if use_0rtt {
        match connecting.into_0rtt() {
            Ok((connection, accepted)) => (connection, Some(accepted)),
            Err(_) => return Err("0-RTT resumption was not available".to_string()),
        }
    } else {
        let connection =
            connecting.await.map_err(|error| format!("quic connect failed: {error}"))?;
        (connection, None)
    };
    let connection_handle = connection.clone();

    let (mut driver, mut send_request) =
        client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(|error| format!("failed to initialize http3 client: {error}"))?;
    let mut driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let mut request_builder = Request::builder()
        .method(method)
        .uri(format!("https://{server_name}:{}{path}", listen_addr.port()));
    for (name, value) in headers {
        request_builder = request_builder.header(*name, *value);
    }
    let mut request_stream = send_request
        .send_request(request_builder.body(()).expect("http3 request should build"))
        .await
        .map_err(|error| format!("failed to send http3 request: {error}"))?;
    if let Some(body) = body {
        request_stream
            .send_data(body)
            .await
            .map_err(|error| format!("failed to send http3 request body: {error}"))?;
    }
    request_stream
        .finish()
        .await
        .map_err(|error| format!("failed to finish http3 request: {error}"))?;

    let response = request_stream
        .recv_response()
        .await
        .map_err(|error| format!("failed to receive http3 response headers: {error}"))?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut body = BytesMut::new();
    while let Some(chunk) = request_stream
        .recv_data()
        .await
        .map_err(|error| format!("failed to receive http3 response body: {error}"))?
    {
        body.extend_from_slice(chunk.chunk());
    }
    let _ = request_stream
        .recv_trailers()
        .await
        .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

    let early_data_accepted = match zero_rtt_accepted {
        Some(accepted) => Some(accepted.await),
        None => None,
    };

    if linger_after_response > Duration::ZERO {
        tokio::time::sleep(linger_after_response).await;
    }

    connection_handle.close(quinn::VarInt::from_u32(0), b"done");

    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }

    Ok((Http3Response { status, headers, body: body.to_vec() }, early_data_accepted))
}

pub(crate) async fn wait_for_http3_0rtt_request(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    timeout: Duration,
) -> Result<(Http3Response, bool), String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match http3_request_with_endpoint(
            endpoint,
            listen_addr,
            server_name,
            "GET",
            path,
            &[],
            None,
            true,
            Duration::ZERO,
        )
        .await
        {
            Ok((response, Some(accepted))) => return Ok((response, accepted)),
            Ok((_response, None)) => {
                last_error =
                    "0-RTT request unexpectedly completed without an acceptance signal".to_string();
            }
            Err(error) if error.contains("0-RTT resumption was not available") => {
                last_error = error;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(format!(
        "timed out waiting for reusable 0-RTT state for https://{server_name}:{}{path}; last error: {last_error}",
        listen_addr.port()
    ))
}

pub(crate) async fn wait_for_http3_0rtt_request_status(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    expected_status: StatusCode,
    timeout: Duration,
) -> Result<(Http3Response, bool), String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match wait_for_http3_0rtt_request(
            endpoint,
            listen_addr,
            server_name,
            path,
            Duration::from_millis(250),
        )
        .await
        {
            Ok((response, accepted)) if response.status == expected_status => {
                return Ok((response, accepted));
            }
            Ok((response, accepted)) => {
                last_error = format!(
                    "0-RTT request completed with status={} accepted={} instead of {}",
                    response.status, accepted, expected_status
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => {
                last_error = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }

    Err(format!(
        "timed out waiting for 0-RTT request with status {} for https://{server_name}:{}{path}; last error: {last_error}",
        expected_status,
        listen_addr.port()
    ))
}

pub(crate) fn https_client(
    cert_pem: &str,
) -> Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
> {
    let roots = root_store_from_pem(cert_pem).expect("root store should build");
    let client_config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("https client should support default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(client_config)
        .https_only()
        .enable_all_versions()
        .build();
    Client::builder(TokioExecutor::new()).build(connector)
}

pub(crate) fn root_store_from_pem(cert_pem: &str) -> Result<RootCertStore, String> {
    let cert = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse certificate PEM: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM did not contain a certificate".to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add(cert).map_err(|error| format!("failed to add root certificate: {error}"))?;
    Ok(roots)
}

pub(crate) fn load_certs_from_path(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    CertificateDer::pem_file_iter(path)
        .map_err(|error| format!("failed to open cert `{}`: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse cert `{}`: {error}", path.display()))
}

pub(crate) fn load_private_key_from_path(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|error| format!("failed to parse key `{}`: {error}", path.display()))
}
