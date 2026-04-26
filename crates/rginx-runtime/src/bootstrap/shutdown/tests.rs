use std::collections::HashMap;
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hyper::http::StatusCode;
use rginx_core::{
    ConfigSnapshot, Listener, ReturnAction, Route, RouteAccessControl, RouteAction, RouteMatcher,
    RuntimeSettings, Server, VirtualHost,
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
                server_header: rginx_core::default_server_header(),
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
                allow_early_data: false,
                request_buffering: rginx_core::RouteBufferingPolicy::Auto,
                response_buffering: rginx_core::RouteBufferingPolicy::Auto,
                compression: rginx_core::RouteCompressionPolicy::Auto,
                compression_min_bytes: None,
                compression_content_types: Vec::new(),
                streaming_response_idle_timeout: None,
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
    let mut admin_task = Some(tokio::spawn(async { Ok::<(), std::io::Error>(()) }));
    let mut health_task = Some(tokio::spawn(async {}));
    let mut ocsp_task = Some(tokio::spawn(async {}));

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
    assert!(admin_task.is_none());
    assert!(health_task.is_none());
    assert!(ocsp_task.is_none());
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
    let mut admin_task = Some(tokio::spawn(async { pending::<std::io::Result<()>>().await }));
    let mut health_task = Some(tokio::spawn(async { pending::<()>().await }));
    let mut ocsp_task = Some(tokio::spawn(async { pending::<()>().await }));

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
    assert!(admin_task.is_none());
    assert!(health_task.is_none());
    assert!(ocsp_task.is_none());
    assert!(active_listener_groups.is_empty());
    assert!(draining_listener_groups.is_empty());
}
