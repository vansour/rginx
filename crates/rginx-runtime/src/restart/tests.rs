use std::env;
use std::net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::os::fd::AsRawFd;
use std::sync::Mutex;

use super::{
    INHERITED_LISTENERS_ENV, InheritedListenerFd, InheritedSocketKind,
    take_inherited_listeners_from_env,
};

static INHERITED_LISTENERS_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn take_inherited_listeners_from_env_returns_empty_when_unset() {
    let _guard = INHERITED_LISTENERS_ENV_LOCK.lock().expect("env lock should not be poisoned");
    unsafe {
        env::remove_var(INHERITED_LISTENERS_ENV);
    }

    let inherited = take_inherited_listeners_from_env().expect("inherited listeners should load");
    assert!(inherited.tcp.is_empty());
    assert!(inherited.udp.is_empty());
}

#[test]
fn take_inherited_listeners_from_env_parses_listener_map() {
    let _guard = INHERITED_LISTENERS_ENV_LOCK.lock().expect("env lock should not be poisoned");
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("listener should bind");
    listener.set_nonblocking(true).expect("listener should support nonblocking mode");
    let listen_addr: SocketAddr = listener.local_addr().expect("listener addr should exist");
    let fd = listener.as_raw_fd();
    std::mem::forget(listener);
    let udp_socket = StdUdpSocket::bind(("127.0.0.1", 0)).expect("udp socket should bind");
    let udp_listen_addr: SocketAddr =
        udp_socket.local_addr().expect("udp socket addr should exist");
    let udp_fd = udp_socket.as_raw_fd();
    std::mem::forget(udp_socket);

    let encoded = serde_json::to_string(&vec![
        InheritedListenerFd { kind: InheritedSocketKind::Tcp, listen_addr, fd },
        InheritedListenerFd {
            kind: InheritedSocketKind::Udp,
            listen_addr: udp_listen_addr,
            fd: udp_fd,
        },
    ])
    .expect("listener map should encode");
    unsafe {
        env::set_var(INHERITED_LISTENERS_ENV, encoded);
    }

    let inherited = take_inherited_listeners_from_env().expect("inherited listeners should load");
    assert_eq!(inherited.tcp.len(), 1);
    assert_eq!(inherited.udp.len(), 1);
    assert!(inherited.tcp.contains_key(&listen_addr));
    assert!(inherited.udp.contains_key(&udp_listen_addr));
    assert_eq!(inherited.udp[&udp_listen_addr].len(), 1);
}
