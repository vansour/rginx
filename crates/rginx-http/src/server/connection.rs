use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity};
use crate::pki::parse_tls_client_identity as parse_pki_client_identity;
use crate::state::TlsHandshakeFailureReason;
use crate::timeout::WriteTimeoutIo;

use super::graceful::{serve_h1_connection_io, serve_h2_connection_io};
use super::proxy_protocol::read_proxy_protocol_source_addr;

const ALPN_H2: &[u8] = b"h2";

#[derive(Clone, Copy)]
pub(super) struct Http1ConnectionOptions {
    pub(super) keep_alive: bool,
    pub(super) max_headers: Option<usize>,
    pub(super) header_read_timeout: Option<std::time::Duration>,
    pub(super) response_write_timeout: Option<std::time::Duration>,
}

pub(super) async fn serve_connection(
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
                        hyper_util::rt::TokioIo::new(stream),
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
                        hyper_util::rt::TokioIo::new(stream),
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
        hyper_util::rt::TokioIo::new(stream),
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

pub(super) fn parse_tls_client_identity<'a>(
    der_chain: impl IntoIterator<Item = &'a [u8]>,
) -> TlsClientIdentity {
    parse_pki_client_identity(der_chain).into()
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
