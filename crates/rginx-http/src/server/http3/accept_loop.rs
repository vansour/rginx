use super::*;

pub async fn serve_http3(
    endpoint: quinn::Endpoint,
    listener_id: String,
    state: crate::state::SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut connections = JoinSet::new();
    let mut draining = *shutdown.borrow();
    if draining {
        endpoint.set_server_config(None);
    }

    loop {
        tokio::select! {
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        tracing::info!("http3 accept loop entering drain mode");
                        draining = true;
                        endpoint.set_server_config(None);
                        if connections.is_empty() {
                            break;
                        }
                    }
                    Ok(()) => continue,
                    Err(_) => {
                        tracing::info!("http3 accept loop entering drain mode because shutdown channel closed");
                        draining = true;
                        endpoint.set_server_config(None);
                        if connections.is_empty() {
                            break;
                        }
                    }
                }
            }
            accepted = endpoint.accept(), if !draining => {
                while let Some(result) = connections.try_join_next() {
                    log_connection_task_result(result);
                }

                let Some(incoming) = accepted else {
                    break;
                };

                let current_listener = state.current_listener(&listener_id).await.expect(
                    "listener id should remain available while http3 accept loop is running",
                );
                if current_listener.http3.as_ref().is_some_and(|http3| http3.retry)
                    && !incoming.remote_address_validated()
                    && incoming.may_retry()
                {
                    let remote_addr = incoming.remote_address();
                    state.record_http3_retry_issued(&listener_id);
                    tracing::info!(
                        remote_addr = %remote_addr,
                        listener = %listener_id,
                        "http3 issuing retry to validate client address"
                    );
                    if let Err(error) = incoming.retry() {
                        state.record_http3_retry_failed(&listener_id);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            %error,
                            "http3 retry failed"
                        );
                    }
                    continue;
                }

                let remote_addr = incoming.remote_address();
                let Some(connection_guard) =
                    state.try_acquire_connection(&listener_id, current_listener.server.max_connections)
                else {
                    state.record_connection_rejected(&listener_id);
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        listener = %listener_id,
                        max_connections = current_listener.server.max_connections,
                        active_connections = state.active_connection_count(),
                        "rejecting downstream http3 connection because server max_connections was reached"
                    );
                    incoming.refuse();
                    continue;
                };
                state.record_connection_accepted(&listener_id);

                let state = state.clone();
                let shutdown = shutdown.clone();
                let listener_id = listener_id.clone();
                connections.spawn(async move {
                    super::connection::serve_http3_connection(
                        incoming,
                        listener_id,
                        state,
                        remote_addr,
                        shutdown,
                        connection_guard,
                    )
                    .await
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = joined {
                    log_connection_task_result(result);
                }
                if draining && connections.is_empty() {
                    break;
                }
            }
            else => {
                if draining || connections.is_empty() {
                    break;
                }
            }
        }
    }

    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }

    endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    endpoint.wait_idle().await;

    tracing::info!("http3 server stopped");
    Ok(())
}

fn log_connection_task_result(result: std::result::Result<Result<()>, JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "http3 connection task failed");
        }
        Err(error) if error.is_panic() => {
            tracing::warn!(%error, "http3 connection task panicked");
        }
        Err(error) if !error.is_cancelled() => {
            tracing::warn!(%error, "http3 connection task failed to join");
        }
        Err(_) => {}
    }
}
