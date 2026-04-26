use super::*;

pub(super) async fn serve_http3_connection(
    incoming: Incoming,
    listener_id: String,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    _connection_guard: crate::state::ActiveConnectionGuard,
) -> Result<()> {
    let current_listener = state
        .current_listener(&listener_id)
        .await
        .expect("listener id should remain available while http3 connection is running");
    let _http3_connection_guard = state.retain_http3_connection(&listener_id)?;
    let mtls_configured =
        current_listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref()).is_some();
    let early_data_enabled =
        current_listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled);
    let connecting = match incoming.accept() {
        Ok(connecting) => connecting,
        Err(error) => {
            let reason = super::super::connection::classify_tls_handshake_failure(&error);
            state.record_tls_handshake_failure(&listener_id, reason);
            tracing::warn!(
                remote_addr = %remote_addr,
                listener = %listener_id,
                tls_handshake_failure = reason.as_str(),
                %error,
                "http3 TLS handshake failed"
            );
            return Ok(());
        }
    };
    let (connection, handshake_complete) = if early_data_enabled && !mtls_configured {
        match connecting.into_0rtt() {
            Ok((connection, zero_rtt_accepted)) => {
                let handshake_complete = Arc::new(AtomicBool::new(false));
                let observed = handshake_complete.clone();
                tokio::spawn(async move {
                    let _ = zero_rtt_accepted.await;
                    observed.store(true, Ordering::Release);
                });
                (connection, handshake_complete)
            }
            Err(connecting) => {
                let connection = match connecting.await {
                    Ok(connection) => connection,
                    Err(error) => {
                        let reason =
                            super::super::connection::classify_tls_handshake_failure(&error);
                        state.record_tls_handshake_failure(&listener_id, reason);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            tls_handshake_failure = reason.as_str(),
                            %error,
                            "http3 TLS handshake failed"
                        );
                        return Ok(());
                    }
                };
                let handshake_complete = Arc::new(AtomicBool::new(true));
                (connection, handshake_complete)
            }
        }
    } else {
        let connection = match connecting.await {
            Ok(connection) => connection,
            Err(error) => {
                let reason = super::super::connection::classify_tls_handshake_failure(&error);
                state.record_tls_handshake_failure(&listener_id, reason);
                tracing::warn!(
                    remote_addr = %remote_addr,
                    listener = %listener_id,
                    tls_handshake_failure = reason.as_str(),
                    %error,
                    "http3 TLS handshake failed"
                );
                return Ok(());
            }
        };
        let handshake_complete = Arc::new(AtomicBool::new(true));
        (connection, handshake_complete)
    };
    let transport_connection = connection.clone();
    let tls_client_identity = extract_http3_tls_client_identity(&transport_connection);
    let tls_alpn = http3_tls_alpn_protocol(&transport_connection);
    let mut request_tasks = JoinSet::new();
    let h3_connection = h3_quinn::Connection::new(connection);
    let mut h3 = H3Connection::new(h3_connection).await.map_err(|error| {
        Error::Server(format!("failed to initialize http3 connection: {error}"))
    })?;
    if mtls_configured {
        state.record_mtls_handshake_success(&listener_id, tls_client_identity.is_some());
    }

    let connection_addrs = Arc::new(ConnectionPeerAddrs {
        socket_peer_addr: remote_addr,
        proxy_protocol_source_addr: None,
        tls_client_identity,
        tls_version: Some("TLS1.3".to_string()),
        tls_alpn: tls_alpn.or_else(|| Some("h3".to_string())),
        early_data: false,
    });

    let mut draining = *shutdown.borrow();
    if draining {
        let _ = h3.shutdown(0).await;
    }

    loop {
        tokio::select! {
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        let _ = h3.shutdown(0).await;
                    }
                    Ok(()) => {}
                    Err(_) => {
                        draining = true;
                        let _ = h3.shutdown(0).await;
                    }
                }
            }
            accepted = h3.accept(), if !draining => {
                match accepted {
                    Ok(Some(resolver)) => {
                        let state = state.clone();
                        let listener_id = listener_id.clone();
                        let connection_addrs = connection_addrs.clone();
                        let handshake_complete = handshake_complete.clone();
                        request_tasks.spawn(async move {
                            let _request_stream_guard =
                                state.retain_http3_request_stream(&listener_id)?;
                            let mut connection_addrs = connection_addrs.as_ref().clone();
                            connection_addrs.early_data = !handshake_complete.load(Ordering::Acquire);
                            super::request::serve_http3_request(
                                resolver,
                                listener_id,
                                state,
                                Arc::new(connection_addrs),
                            )
                            .await
                        });
                    }
                    Ok(None) => break,
                    Err(error) => {
                        if super::close_reason::is_clean_http3_accept_close(&error) {
                            tracing::debug!(%error, "http3 peer closed connection cleanly");
                            break;
                        }
                        state.record_http3_request_accept_error(&listener_id);
                        return Err(Error::Server(format!("http3 request accept failed: {error}")));
                    }
                }
            }
            joined = request_tasks.join_next(), if !request_tasks.is_empty() => {
                if let Some(result) = joined {
                    log_request_task_result(result);
                }
                if draining && request_tasks.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    break;
                }
            }
            else => {
                if request_tasks.is_empty() {
                    break;
                }
            }
        }
    }

    while let Some(result) = request_tasks.join_next().await {
        log_request_task_result(result);
    }

    let close_reason = transport_connection.closed().await;
    state.record_http3_connection_close(&listener_id, close_reason);

    Ok(())
}

fn extract_http3_tls_client_identity(connection: &quinn::Connection) -> Option<TlsClientIdentity> {
    let certificates = connection
        .peer_identity()?
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    Some(super::super::connection::parse_tls_client_identity(
        certificates.iter().map(|certificate| certificate.as_ref()),
    ))
}

fn http3_tls_alpn_protocol(connection: &quinn::Connection) -> Option<String> {
    connection
        .handshake_data()?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .ok()
        .and_then(|data| data.protocol)
        .map(|protocol| String::from_utf8_lossy(&protocol).into_owned())
}

fn log_request_task_result(result: std::result::Result<Result<()>, JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "http3 request task failed");
        }
        Err(error) if error.is_panic() => {
            tracing::warn!(%error, "http3 request task panicked");
        }
        Err(error) if !error.is_cancelled() => {
            tracing::warn!(%error, "http3 request task failed to join");
        }
        Err(_) => {}
    }
}
