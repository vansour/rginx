use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rginx_core::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio_rustls::TlsAcceptor;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity};
use crate::state::TlsHandshakeFailureReason;
use crate::timeout::WriteTimeoutIo;

const ALPN_H2: &[u8] = b"h2";

#[derive(Clone, Copy)]
struct Http1ConnectionOptions {
    keep_alive: bool,
    max_headers: Option<usize>,
    header_read_timeout: Option<std::time::Duration>,
    response_write_timeout: Option<std::time::Duration>,
}

pub async fn serve(
    listener: TcpListener,
    listener_id: String,
    state: crate::state::SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut connections = JoinSet::new();

    {
        let listener = listener;

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if *shutdown.borrow() => {
                            tracing::info!(
                                active_connections = state.active_connection_count(),
                                "http accept loop stopping"
                            );
                            break;
                        }
                        Ok(()) => continue,
                        Err(_) => {
                            tracing::info!(
                                active_connections = state.active_connection_count(),
                                "http accept loop stopping because shutdown channel closed"
                            );
                            break;
                        }
                    }
                }
                accepted = listener.accept() => {
                    while let Some(result) = connections.try_join_next() {
                        log_connection_task_result(result);
                    }

                    let (stream, remote_addr) = accepted?;
                    let state = state.clone();
                    let shutdown = shutdown.clone();
                    let tls_acceptor = state.tls_acceptor(&listener_id).await;
                    let current_listener = state.current_listener(&listener_id).await.expect(
                        "listener id should remain available while accept loop is running",
                    );
                    let Some(connection_guard) =
                        state.try_acquire_connection(
                            &listener_id,
                            current_listener.server.max_connections,
                        )
                    else {
                        state.record_connection_rejected(&listener_id);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            max_connections = current_listener.server.max_connections,
                            active_connections = state.active_connection_count(),
                            "rejecting downstream connection because server max_connections was reached"
                        );
                        drop(stream);
                        continue;
                    };
                    state.record_connection_accepted(&listener_id);
                    let http1 = Http1ConnectionOptions {
                        keep_alive: current_listener.server.keep_alive,
                        max_headers: current_listener.server.max_headers,
                        header_read_timeout: current_listener.server.header_read_timeout,
                        response_write_timeout: current_listener.server.response_write_timeout,
                    };
                    let connection_listener_id = listener_id.clone();

                    connections.spawn(async move {
                        serve_connection(
                            stream,
                            connection_listener_id,
                            state,
                            remote_addr,
                            shutdown,
                            tls_acceptor,
                            http1,
                            connection_guard,
                        )
                        .await;
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(result) = joined {
                        log_connection_task_result(result);
                    }
                }
            }
        }
    }

    if !connections.is_empty() {
        tracing::info!(
            active_connections = state.active_connection_count(),
            "waiting for active connections to drain"
        );
    }

    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }

    tracing::info!("http server stopped");

    Ok(())
}

async fn serve_connection(
    mut stream: tokio::net::TcpStream,
    listener_id: String,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    shutdown: watch::Receiver<bool>,
    tls_acceptor: Option<TlsAcceptor>,
    http1: Http1ConnectionOptions,
    _connection_guard: crate::state::ActiveConnectionGuard,
) {
    let current_listener = state
        .current_listener(&listener_id)
        .await
        .expect("listener id should remain available while connection is running");
    let proxy_protocol_source_addr = if current_listener.proxy_protocol_enabled {
        match read_proxy_protocol_source_addr(
            &mut stream,
            remote_addr,
            current_listener.server.is_trusted_proxy(remote_addr.ip()),
        )
        .await
        {
            Ok(source_addr) => source_addr,
            Err(error) => {
                tracing::warn!(
                    remote_addr = %remote_addr,
                    listener = %listener_id,
                    %error,
                    "failed to parse proxy protocol header"
                );
                return;
            }
        }
    } else {
        None
    };
    let connection_addrs = ConnectionPeerAddrs {
        socket_peer_addr: remote_addr,
        proxy_protocol_source_addr,
        tls_client_identity: None,
        tls_version: None,
        tls_alpn: None,
    };

    if let Some(tls_acceptor) = tls_acceptor {
        match tls_acceptor.accept(stream).await {
            Ok(stream) => {
                let tls_client_identity = extract_tls_client_identity(&stream);
                let mtls_configured = current_listener
                    .server
                    .tls
                    .as_ref()
                    .and_then(|tls| tls.client_auth.as_ref())
                    .is_some();
                if mtls_configured {
                    state
                        .record_mtls_handshake_success(&listener_id, tls_client_identity.is_some());
                }
                let connection_addrs = ConnectionPeerAddrs {
                    tls_client_identity,
                    tls_version: tls_protocol_version(&stream),
                    tls_alpn: tls_alpn_protocol(&stream),
                    ..connection_addrs
                };
                let connection_addrs = Arc::new(connection_addrs);
                if negotiated_h2(&stream) {
                    let stream = WriteTimeoutIo::new(
                        stream,
                        http1.response_write_timeout,
                        format!("downstream response to {remote_addr}"),
                    );
                    serve_h2_connection_io(
                        TokioIo::new(stream),
                        listener_id,
                        state,
                        connection_addrs,
                        shutdown,
                    )
                    .await;
                } else {
                    let stream = WriteTimeoutIo::new(
                        stream,
                        http1.response_write_timeout,
                        format!("downstream response to {remote_addr}"),
                    );
                    serve_h1_connection_io(
                        TokioIo::new(stream),
                        listener_id,
                        state,
                        connection_addrs,
                        shutdown,
                        http1,
                    )
                    .await;
                }
            }
            Err(error) => {
                let reason = classify_tls_handshake_failure(&error);
                state.record_tls_handshake_failure(&listener_id, reason);
                tracing::warn!(
                    remote_addr = %remote_addr,
                    listener = %listener_id,
                    tls_handshake_failure = reason.as_str(),
                    %error,
                    "TLS handshake failed"
                );
            }
        }
        return;
    }

    let stream = WriteTimeoutIo::new(
        stream,
        http1.response_write_timeout,
        format!("downstream response to {remote_addr}"),
    );
    let connection_addrs = Arc::new(connection_addrs);
    serve_h1_connection_io(
        TokioIo::new(stream),
        listener_id,
        state,
        connection_addrs,
        shutdown,
        http1,
    )
    .await;
}

fn negotiated_h2(stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>) -> bool {
    stream.get_ref().1.alpn_protocol() == Some(ALPN_H2)
}

fn extract_tls_client_identity(
    stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Option<TlsClientIdentity> {
    let certs = stream.get_ref().1.peer_certificates()?;
    Some(parse_tls_client_identity(certs.iter().map(|cert| cert.as_ref())))
}

fn parse_tls_client_identity<'a>(
    der_chain: impl IntoIterator<Item = &'a [u8]>,
) -> TlsClientIdentity {
    let mut identity = TlsClientIdentity {
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: Vec::new(),
        chain_length: 0,
        chain_subjects: Vec::new(),
    };

    for (index, der) in der_chain.into_iter().enumerate() {
        identity.chain_length += 1;
        if let Ok((_, cert)) = X509Certificate::from_der(der) {
            let subject = format!("{}", cert.subject());
            identity.chain_subjects.push(subject.clone());
            if index == 0 {
                identity.subject = Some(subject);
                identity.issuer = Some(format!("{}", cert.issuer()));
                identity.serial_number = Some(cert.tbs_certificate.raw_serial_as_string());
                if let Ok(Some(san)) = cert.subject_alternative_name() {
                    identity.san_dns_names = san
                        .value
                        .general_names
                        .iter()
                        .filter_map(|name| match name {
                            GeneralName::DNSName(dns) => Some(dns.to_string()),
                            _ => None,
                        })
                        .collect();
                }
            }
        }
    }

    identity
}

fn tls_protocol_version(
    stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Option<String> {
    stream.get_ref().1.protocol_version().map(|version| match version {
        rustls::ProtocolVersion::TLSv1_2 => "TLS1.2".to_string(),
        rustls::ProtocolVersion::TLSv1_3 => "TLS1.3".to_string(),
        other => format!("{other:?}"),
    })
}

fn tls_alpn_protocol(
    stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Option<String> {
    stream
        .get_ref()
        .1
        .alpn_protocol()
        .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
}

fn classify_tls_handshake_failure(error: &impl std::fmt::Display) -> TlsHandshakeFailureReason {
    let error = error.to_string().to_ascii_lowercase();
    if error.contains("certificate required")
        || error.contains("peer sent no certificates")
        || error.contains("no certificates presented")
    {
        return TlsHandshakeFailureReason::MissingClientCert;
    }
    if error.contains("unknown ca") || error.contains("unknown issuer") {
        return TlsHandshakeFailureReason::UnknownCa;
    }
    if error.contains("revoked") {
        return TlsHandshakeFailureReason::CertificateRevoked;
    }
    if error.contains("verify_depth") || error.contains("chain exceeds configured verify_depth") {
        return TlsHandshakeFailureReason::VerifyDepthExceeded;
    }
    if error.contains("bad certificate")
        || error.contains("certificate verify failed")
        || error.contains("invalid peer certificate")
    {
        return TlsHandshakeFailureReason::BadCertificate;
    }
    TlsHandshakeFailureReason::Other
}

async fn serve_h1_connection_io<T>(
    io: TokioIo<T>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
    mut shutdown: watch::Receiver<bool>,
    options: Http1ConnectionOptions,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service_connection_addrs = connection_addrs.clone();
    let service = service_fn(move |request| {
        let state = state.clone();
        let listener_id = listener_id.clone();
        let connection_addrs = service_connection_addrs.clone();
        async move {
            Ok::<_, Infallible>(
                crate::handler::handle(request, state, connection_addrs, &listener_id).await,
            )
        }
    });

    let mut builder = http1::Builder::new();
    builder.keep_alive(options.keep_alive);
    if let Some(max_headers) = options.max_headers {
        builder.max_headers(max_headers);
    }
    if let Some(header_read_timeout) = options.header_read_timeout {
        builder.timer(TokioTimer::new());
        builder.header_read_timeout(header_read_timeout);
    }
    let connection = builder.serve_connection(io, service).with_upgrades();
    tokio::pin!(connection);

    let mut draining = *shutdown.borrow();
    if draining {
        connection.as_mut().graceful_shutdown();
    }

    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    tracing::warn!(remote_addr = %connection_addrs.socket_peer_addr, %error, "connection closed with error");
                }
                break;
            }
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        tracing::debug!(remote_addr = %connection_addrs.socket_peer_addr, "starting graceful shutdown for connection");
                        connection.as_mut().graceful_shutdown();
                    }
                    Ok(()) => {}
                    Err(_) => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
        }
    }
}

async fn serve_h2_connection_io<T>(
    io: TokioIo<T>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
    mut shutdown: watch::Receiver<bool>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service_connection_addrs = connection_addrs.clone();
    let service = service_fn(move |request| {
        let state = state.clone();
        let listener_id = listener_id.clone();
        let connection_addrs = service_connection_addrs.clone();
        async move {
            Ok::<_, Infallible>(
                crate::handler::handle(request, state, connection_addrs, &listener_id).await,
            )
        }
    });

    let connection = http2::Builder::new(TokioExecutor::new()).serve_connection(io, service);
    tokio::pin!(connection);

    let mut draining = *shutdown.borrow();
    if draining {
        connection.as_mut().graceful_shutdown();
    }

    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    tracing::warn!(
                        remote_addr = %connection_addrs.socket_peer_addr,
                        %error,
                        "http2 connection closed with error"
                    );
                }
                break;
            }
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        tracing::debug!(
                            remote_addr = %connection_addrs.socket_peer_addr,
                            "starting graceful shutdown for http2 connection"
                        );
                        connection.as_mut().graceful_shutdown();
                    }
                    Ok(()) => {}
                    Err(_) => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
        }
    }
}

const MAX_PROXY_PROTOCOL_HEADER_BYTES: usize = 108;

async fn read_proxy_protocol_source_addr(
    stream: &mut tokio::net::TcpStream,
    remote_addr: SocketAddr,
    trust_remote_addr: bool,
) -> std::io::Result<Option<SocketAddr>> {
    let mut header = Vec::with_capacity(MAX_PROXY_PROTOCOL_HEADER_BYTES);
    loop {
        if header.len() >= MAX_PROXY_PROTOCOL_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proxy protocol header is too long",
            ));
        }

        let byte = stream.read_u8().await?;
        header.push(byte);
        if header.ends_with(b"\r\n") {
            break;
        }
    }

    let header = std::str::from_utf8(&header).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "proxy protocol header is not valid utf-8",
        )
    })?;
    parse_proxy_protocol_v1(header, remote_addr, trust_remote_addr)
}

fn parse_proxy_protocol_v1(
    header: &str,
    remote_addr: SocketAddr,
    trust_remote_addr: bool,
) -> std::io::Result<Option<SocketAddr>> {
    let header = header.trim_end_matches("\r\n");
    if header == "PROXY UNKNOWN" {
        return Ok(None);
    }

    let mut parts = header.split_whitespace();
    let prefix = parts.next();
    let protocol = parts.next();
    let source_addr = parts.next();
    let _destination_addr = parts.next();
    let source_port = parts.next();
    let _destination_port = parts.next();
    let trailing = parts.next();

    if prefix != Some("PROXY") || trailing.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid proxy protocol header",
        ));
    }

    let source = match protocol {
        Some("TCP4") | Some("TCP6") => {
            let ip = source_addr
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing proxy protocol source address",
                    )
                })?
                .parse::<std::net::IpAddr>()
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid proxy protocol source address",
                    )
                })?;
            let port = source_port
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "missing proxy protocol source port",
                    )
                })?
                .parse::<u16>()
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid proxy protocol source port",
                    )
                })?;
            Some(SocketAddr::new(ip, port))
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported proxy protocol transport",
            ));
        }
    };

    if !trust_remote_addr {
        tracing::warn!(
            remote_addr = %remote_addr,
            "ignoring proxy protocol header because the transport peer is not trusted"
        );
        return Ok(None);
    }

    Ok(source)
}

fn log_connection_task_result(result: std::result::Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_panic() {
            tracing::warn!(%error, "connection task panicked");
        } else if !error.is_cancelled() {
            tracing::warn!(%error, "connection task failed to join");
        }
    }
}

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, DnType, KeyPair};
    use rustls_pemfile::certs;

    use super::{parse_proxy_protocol_v1, parse_tls_client_identity};

    #[test]
    fn proxy_protocol_v1_parses_tcp4_source_address() {
        let source = parse_proxy_protocol_v1(
            "PROXY TCP4 198.51.100.9 203.0.113.10 12345 443\r\n",
            "10.0.0.1:4000".parse().unwrap(),
            true,
        )
        .expect("header should parse");

        assert_eq!(source, Some("198.51.100.9:12345".parse().unwrap()));
    }

    #[test]
    fn proxy_protocol_v1_accepts_unknown_transport() {
        let source =
            parse_proxy_protocol_v1("PROXY UNKNOWN\r\n", "10.0.0.1:4000".parse().unwrap(), true)
                .expect("unknown header should parse");

        assert_eq!(source, None);
    }

    #[test]
    fn proxy_protocol_v1_rejects_invalid_headers() {
        let error = parse_proxy_protocol_v1("BROKEN\r\n", "10.0.0.1:4000".parse().unwrap(), true)
            .expect_err("invalid header should fail");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn parse_tls_client_identity_extracts_subject_and_dns_san() {
        let mut params = CertificateParams::new(vec!["localhost".to_string()])
            .expect("certificate params should build");
        params.distinguished_name.push(DnType::CommonName, "localhost");
        let key_pair = KeyPair::generate().expect("keypair should generate");
        let cert = params.self_signed(&key_pair).expect("cert should generate");
        let pem = cert.pem();
        let mut reader = std::io::Cursor::new(pem.as_bytes());
        let cert = certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .expect("certificate PEM should parse")
            .remove(0);

        let identity = parse_tls_client_identity(std::iter::once(cert.as_ref()));

        assert!(
            identity.subject.as_deref().is_some_and(|subject| subject.contains("CN=localhost"))
        );
        assert!(identity.issuer.as_deref().is_some_and(|issuer| issuer.contains("CN=localhost")));
        assert!(identity.serial_number.is_some());
        assert!(identity.san_dns_names.iter().any(|san| san == "localhost"));
        assert_eq!(identity.chain_length, 1);
        assert_eq!(identity.chain_subjects.len(), 1);
    }
}
