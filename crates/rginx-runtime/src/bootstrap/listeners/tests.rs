use std::collections::HashMap;
use std::net::SocketAddr;
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Listener, Server};
use tokio::sync::watch;

use super::*;

fn listener(id: &str, name: &str, listen_addr: SocketAddr) -> Listener {
    Listener {
        id: id.to_string(),
        name: name.to_string(),
        server: Server {
            listen_addr,
            server_header: rginx_core::default_server_header(),
            default_certificate: None,
            trusted_proxies: Vec::new(),
            client_ip_header: None,
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
    }
}

fn config_with_listeners(listeners: Vec<Listener>) -> ConfigSnapshot {
    ConfigSnapshot {
        cache_zones: std::collections::HashMap::new(),
        runtime: rginx_core::RuntimeSettings {
            shutdown_timeout: std::time::Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners,
        default_vhost: rginx_core::VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        },
        vhosts: Vec::new(),
        upstreams: HashMap::new(),
    }
}

fn listener_group_with_socket(
    listener: Listener,
    std_listener: StdTcpListener,
) -> ListenerWorkerGroup {
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    ListenerWorkerGroup {
        listener,
        std_listener: Arc::new(std_listener),
        std_udp_sockets: Vec::new(),
        shutdown_tx,
        tasks: Vec::new(),
        joined_tasks: 0,
    }
}

#[test]
fn prepare_added_listener_bindings_rejects_active_addr_reuse_with_new_id() {
    let std_listener =
        bind_std_listener("127.0.0.1:0".parse().expect("socket addr should parse")).unwrap();
    let listen_addr = std_listener.local_addr().expect("listener addr should exist");

    let active_listener = listener("listener-a", "listener-a", listen_addr);
    let active_groups = HashMap::from([(
        active_listener.id.clone(),
        listener_group_with_socket(active_listener, std_listener),
    )]);
    let next_config =
        config_with_listeners(vec![listener("listener-b", "listener-b", listen_addr)]);
    let error = match prepare_added_listener_bindings(
        &next_config,
        &[listener("listener-b", "listener-b", listen_addr)],
        1,
        &active_groups,
        &[],
    ) {
        Ok(_) => panic!("reusing an active listen addr with a new id must fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("reuses tcp listen address"));
}

#[test]
fn prepare_added_listener_bindings_rejects_draining_addr_reuse_with_new_id() {
    let std_listener =
        bind_std_listener("127.0.0.1:0".parse().expect("socket addr should parse")).unwrap();
    let listen_addr = std_listener.local_addr().expect("listener addr should exist");

    let draining_groups = vec![listener_group_with_socket(
        listener("listener-a", "listener-a", listen_addr),
        std_listener,
    )];
    let next_config =
        config_with_listeners(vec![listener("listener-b", "listener-b", listen_addr)]);
    let error = match prepare_added_listener_bindings(
        &next_config,
        &[listener("listener-b", "listener-b", listen_addr)],
        1,
        &HashMap::new(),
        &draining_groups,
    ) {
        Ok(_) => panic!("reusing a draining listen addr with a new id must fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("reuses tcp listen address"));
}

#[test]
fn bind_std_udp_sockets_creates_one_socket_per_worker() {
    let seed = bind_std_udp_socket("127.0.0.1:0".parse().expect("socket addr should parse"), false)
        .expect("seed udp socket should bind");
    let listen_addr = seed.local_addr().expect("seed udp addr should exist");
    drop(seed);

    let sockets = bind_std_udp_sockets(listen_addr, 3).expect("udp sockets should bind");
    assert_eq!(sockets.len(), 3);
    for socket in sockets {
        assert_eq!(socket.local_addr().expect("udp socket addr should exist"), listen_addr);
    }
}

#[test]
fn normalize_inherited_udp_sockets_truncates_to_worker_count() {
    let seed = bind_std_udp_socket("127.0.0.1:0".parse().expect("socket addr should parse"), false)
        .expect("seed udp socket should bind");
    let listen_addr = seed.local_addr().expect("seed udp addr should exist");
    drop(seed);

    let inherited = (0..3)
        .map(|_| bind_std_udp_socket(listen_addr, true).expect("inherited udp socket should bind"))
        .collect::<Vec<_>>();

    let sockets = normalize_inherited_udp_sockets("default", listen_addr, inherited, 2)
        .expect("normalizing inherited sockets should succeed");
    assert_eq!(sockets.len(), 2);
    for socket in sockets {
        assert_eq!(socket.local_addr().expect("udp socket addr should exist"), listen_addr);
    }
}

#[test]
fn normalize_inherited_udp_sockets_fills_missing_workers() {
    let seed = bind_std_udp_socket("127.0.0.1:0".parse().expect("socket addr should parse"), false)
        .expect("seed udp socket should bind");
    let listen_addr = seed.local_addr().expect("seed udp addr should exist");
    drop(seed);

    let inherited = (0..2)
        .map(|_| bind_std_udp_socket(listen_addr, true).expect("inherited udp socket should bind"))
        .collect::<Vec<_>>();

    let sockets = normalize_inherited_udp_sockets("default", listen_addr, inherited, 3)
        .expect("normalizing inherited sockets should succeed");
    assert_eq!(sockets.len(), 3);
    for socket in sockets {
        assert_eq!(socket.local_addr().expect("udp socket addr should exist"), listen_addr);
    }
}

#[test]
fn normalize_inherited_udp_sockets_rejects_one_to_many_restart() {
    let inherited = vec![
        bind_std_udp_socket("127.0.0.1:0".parse().expect("socket addr should parse"), false)
            .expect("inherited udp socket should bind"),
    ];
    let listen_addr = inherited[0].local_addr().expect("udp socket addr should exist");

    let error = normalize_inherited_udp_sockets("default", listen_addr, inherited, 3)
        .expect_err("one-to-many restart should fail");
    assert!(error.to_string().contains("cannot increase HTTP/3 accept_workers from 1 to 3"));
}
