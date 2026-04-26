use super::*;

pub(crate) async fn spawn_grpc_upstream()
-> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::Immediate).await
}

pub(crate) async fn spawn_grpc_upstream_with_response_delay(
    response_delay: Duration,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::DelayHeaders(response_delay)).await
}

pub(crate) async fn spawn_grpc_upstream_with_body_delay(
    body_delay: Duration,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::DelayBody(body_delay)).await
}

pub(crate) async fn spawn_grpc_upstream_with_dynamic_health(
    health_status: Arc<AtomicU8>,
) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-health-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        loop {
            let stream = tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        break;
                    };
                    stream
                }
            };
            let tls_stream =
                tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");

            let health_status = health_status.clone();
            let service = service_fn(move |request: Request<Incoming>| {
                let health_status = health_status.clone();

                async move {
                    let path = request.uri().path().to_string();
                    let response = if path == GRPC_METHOD_PATH {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .header("grpc-status", "0")
                            .body(EitherGrpcResponseBody::Full(Full::new(
                                grpc_health_response_frame(health_status.load(Ordering::Relaxed)),
                            )))
                            .expect("gRPC health response should build")
                    } else if path == APP_GRPC_METHOD_PATH {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .header("grpc-status", "0")
                            .body(EitherGrpcResponseBody::Full(Full::new(Bytes::from_static(
                                GRPC_RESPONSE_FRAME,
                            ))))
                            .expect("upstream gRPC response should build")
                    } else {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .body(EitherGrpcResponseBody::Immediate(GrpcResponseBody::new()))
                            .expect("upstream gRPC response should build")
                    };

                    Ok::<_, Infallible>(response)
                }
            });

            let _ = http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tls_stream), service)
                .await;
        }
    });

    (listen_addr, shutdown_tx, task, temp_dir)
}

pub(crate) async fn spawn_cancellable_grpc_upstream()
-> (SocketAddr, oneshot::Receiver<()>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-cancel-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (cancelled_tx, cancelled_rx) = oneshot::channel();
    let cancelled_tx = Arc::new(Mutex::new(Some(cancelled_tx)));

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream listener should accept");
        let tls_stream =
            tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");

        let service = service_fn(move |request: Request<Incoming>| {
            let cancelled_tx = cancelled_tx.clone();
            async move {
                assert_eq!(request.uri().path(), APP_GRPC_METHOD_PATH);
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/grpc")
                        .body(EitherGrpcResponseBody::Cancellable(
                            CancellableGrpcResponseBody::new(cancelled_tx),
                        ))
                        .expect("cancellable gRPC response should build"),
                )
            }
        });

        http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream gRPC h2 connection should complete");
    });

    (listen_addr, cancelled_rx, task, temp_dir)
}

pub(crate) async fn spawn_grpc_upstream_with_mode(
    mode: UpstreamResponseMode,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (observed_tx, observed_rx) = oneshot::channel();
    let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream listener should accept");
        let tls_stream =
            tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");
        let alpn_protocol = tls_stream
            .get_ref()
            .1
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned());

        let service = service_fn(move |request: Request<Incoming>| {
            let observed_tx = observed_tx.clone();
            let alpn_protocol = alpn_protocol.clone();

            async move {
                let (parts, body) = request.into_parts();
                let (body_bytes, trailers) = read_body_and_trailers(body).await;

                if let Some(sender) =
                    observed_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
                {
                    let _ = sender.send(ObservedRequest {
                        method: parts.method.as_str().to_string(),
                        version: parts.version,
                        path: parts.uri.path().to_string(),
                        alpn_protocol,
                        content_type: parts
                            .headers
                            .get(CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        grpc_timeout: parts
                            .headers
                            .get("grpc-timeout")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        te: parts
                            .headers
                            .get(TE)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        body: body_bytes.freeze(),
                        trailers,
                    });
                }

                if let UpstreamResponseMode::DelayHeaders(response_delay) = mode
                    && !response_delay.is_zero()
                {
                    tokio::time::sleep(response_delay).await;
                }

                let body = match mode {
                    UpstreamResponseMode::Immediate | UpstreamResponseMode::DelayHeaders(_) => {
                        EitherGrpcResponseBody::Immediate(GrpcResponseBody::new())
                    }
                    UpstreamResponseMode::DelayBody(body_delay) => {
                        EitherGrpcResponseBody::Delayed(DelayedGrpcResponseBody::new(body_delay))
                    }
                };

                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/grpc")
                        .body(body)
                        .expect("upstream gRPC response should build"),
                )
            }
        });

        http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream gRPC h2 connection should complete");
    });

    (listen_addr, observed_rx, task, temp_dir)
}

pub(crate) async fn read_body_and_trailers(body: Incoming) -> (BytesMut, Option<HeaderMap>) {
    let mut body = body;
    let mut bytes = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame = frame.expect("response frame should succeed");
        match frame.into_data() {
            Ok(data) => bytes.extend_from_slice(&data),
            Err(frame) => match frame.into_trailers() {
                Ok(frame_trailers) => trailers = Some(frame_trailers),
                Err(_) => panic!("unexpected non-data, non-trailers frame"),
            },
        }
    }

    (bytes, trailers)
}
