use std::time::Duration;

use rginx_core::{Error, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::state::RuntimeState;

use super::listeners::{
    ListenerGroupMap, ListenerWorkerGroup, abort_listener_worker_groups,
    initiate_shutdown_for_groups, join_aborted_listener_worker_groups, join_listener_worker_groups,
};

pub(super) async fn graceful_shutdown(
    state: &RuntimeState,
    shutdown_timeout: Duration,
    shutdown_tx: &watch::Sender<bool>,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
    admin_task: &mut JoinHandle<std::io::Result<()>>,
    health_task: &mut JoinHandle<()>,
    ocsp_task: &mut JoinHandle<()>,
) -> Result<()> {
    let _ = shutdown_tx.send(true);
    initiate_shutdown_for_groups(active_listener_groups.values());
    initiate_shutdown_for_groups(draining_listener_groups.iter());

    match tokio::time::timeout(shutdown_timeout, async {
        join_listener_worker_groups(&state.http, active_listener_groups, draining_listener_groups)
            .await?;
        (&mut *admin_task).await.map_err(|error| {
            Error::Server(format!("admin socket task failed to join: {error}"))
        })??;
        (&mut *health_task).await.map_err(|error| {
            Error::Server(format!("active health task failed to join: {error}"))
        })?;
        (&mut *ocsp_task)
            .await
            .map_err(|error| Error::Server(format!("OCSP refresh task failed to join: {error}")))?;
        state.http.drain_background_tasks().await;
        Ok::<(), Error>(())
    })
    .await
    {
        Ok(join_result) => join_result,
        Err(_) => {
            tracing::warn!(
                "shutdown timeout reached before background tasks drained all active work"
            );
            abort_listener_worker_groups(active_listener_groups.values());
            abort_listener_worker_groups(draining_listener_groups.iter());
            admin_task.abort();
            health_task.abort();
            ocsp_task.abort();

            join_aborted_listener_worker_groups(
                &state.http,
                active_listener_groups,
                draining_listener_groups,
            )
            .await;

            if let Err(error) = (&mut *admin_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "admin socket task failed after abort");
            }

            if let Err(error) = (&mut *health_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "active health task failed after abort");
            }

            if let Err(error) = (&mut *ocsp_task).await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "OCSP refresh task failed after abort");
            }

            state.http.abort_background_tasks().await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::pending;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use hyper::http::StatusCode;
    use rginx_core::{
        ConfigSnapshot, Listener, ReturnAction, Route, RouteAccessControl, RouteAction,
        RouteMatcher, RuntimeSettings, Server, VirtualHost,
    };
    use tokio::sync::watch;

    use super::*;

    fn snapshot() -> ConfigSnapshot {
        ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            listeners: vec![Listener {
                id: "default".to_string(),
                name: "default".to_string(),
                server: Server {
                    listen_addr: "127.0.0.1:0".parse().expect("socket addr should parse"),
                    default_certificate: None,
                    trusted_proxies: Vec::new(),
                    keep_alive: true,
                    max_headers: None,
                    max_request_body_bytes: None,
                    max_connections: None,
                    header_read_timeout: None,
                    request_body_read_timeout: None,
                    response_write_timeout: None,
                    access_log_format: None,
                    tls: None,
                },
                tls_termination_enabled: false,
                proxy_protocol_enabled: false,
                http3: None,
            }],
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: vec![Route {
                    id: "server/routes[0]|exact:/".to_string(),
                    matcher: RouteMatcher::Exact("/".to_string()),
                    grpc_match: None,
                    action: RouteAction::Return(ReturnAction {
                        status: StatusCode::OK,
                        location: String::new(),
                        body: Some("ok\n".to_string()),
                    }),
                    access_control: RouteAccessControl::default(),
                    rate_limit: None,
                }],
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn graceful_shutdown_waits_for_background_tasks_and_signals_shutdown() {
        let state = RuntimeState::new(PathBuf::from("/tmp/rginx-shutdown-test.ron"), snapshot())
            .expect("runtime state should build");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let background_task_drained = Arc::new(AtomicBool::new(false));
        let drained = background_task_drained.clone();
        state.http.spawn_background_task(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drained.store(true, Ordering::Relaxed);
        });

        let mut active_listener_groups = ListenerGroupMap::new();
        let mut draining_listener_groups = Vec::new();
        let mut admin_task = tokio::spawn(async { Ok::<(), std::io::Error>(()) });
        let mut health_task = tokio::spawn(async {});
        let mut ocsp_task = tokio::spawn(async {});

        graceful_shutdown(
            &state,
            Duration::from_millis(200),
            &shutdown_tx,
            &mut active_listener_groups,
            &mut draining_listener_groups,
            &mut admin_task,
            &mut health_task,
            &mut ocsp_task,
        )
        .await
        .expect("graceful shutdown should succeed");

        assert!(*shutdown_rx.borrow());
        assert!(background_task_drained.load(Ordering::Relaxed));
        assert!(admin_task.is_finished());
        assert!(health_task.is_finished());
        assert!(ocsp_task.is_finished());
        assert!(active_listener_groups.is_empty());
        assert!(draining_listener_groups.is_empty());
    }

    #[tokio::test]
    async fn graceful_shutdown_aborts_pending_tasks_after_timeout() {
        let state = RuntimeState::new(PathBuf::from("/tmp/rginx-shutdown-timeout.ron"), snapshot())
            .expect("runtime state should build");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let background_task_started = Arc::new(AtomicBool::new(false));
        let started = background_task_started.clone();
        state.http.spawn_background_task(async move {
            started.store(true, Ordering::Relaxed);
            pending::<()>().await;
        });

        let mut active_listener_groups = ListenerGroupMap::new();
        let mut draining_listener_groups = Vec::new();
        let mut admin_task = tokio::spawn(async { pending::<std::io::Result<()>>().await });
        let mut health_task = tokio::spawn(async { pending::<()>().await });
        let mut ocsp_task = tokio::spawn(async { pending::<()>().await });

        tokio::task::yield_now().await;
        graceful_shutdown(
            &state,
            Duration::from_millis(20),
            &shutdown_tx,
            &mut active_listener_groups,
            &mut draining_listener_groups,
            &mut admin_task,
            &mut health_task,
            &mut ocsp_task,
        )
        .await
        .expect("timeout branch should still resolve successfully");

        assert!(*shutdown_rx.borrow());
        assert!(background_task_started.load(Ordering::Relaxed));
        assert!(admin_task.is_finished());
        assert!(health_task.is_finished());
        assert!(ocsp_task.is_finished());
        assert!(active_listener_groups.is_empty());
        assert!(draining_listener_groups.is_empty());
    }
}
