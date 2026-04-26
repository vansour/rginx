use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
use std::os::fd::AsRawFd;
use std::sync::Arc;

use rginx_core::{Error, Result};
use socket2::{Domain, Protocol, Socket, Type};

pub(crate) fn bind_std_udp_sockets(
    listen_addr: SocketAddr,
    count: usize,
) -> Result<Vec<Arc<StdUdpSocket>>> {
    let count = count.max(1);
    bind_std_udp_sockets_with_reuse_port(listen_addr, count, count > 1)
}

pub(crate) fn normalize_inherited_udp_sockets(
    listener_name: &str,
    listen_addr: SocketAddr,
    sockets: Vec<StdUdpSocket>,
    desired_socket_count: usize,
) -> Result<Vec<Arc<StdUdpSocket>>> {
    let desired_socket_count = desired_socket_count.max(1);
    let mut sockets = sockets.into_iter().map(Arc::new).collect::<Vec<_>>();
    if sockets.len() == 1 && desired_socket_count > 1 {
        return Err(Error::Server(format!(
            "listener `{listener_name}` cannot increase HTTP/3 accept_workers from 1 to {} during restart; perform a cold restart or keep the previous worker count",
            desired_socket_count,
        )));
    }
    if sockets.len() > desired_socket_count {
        sockets.truncate(desired_socket_count);
    } else if sockets.len() < desired_socket_count {
        sockets.extend(bind_std_udp_sockets_with_reuse_port(
            listen_addr,
            desired_socket_count - sockets.len(),
            desired_socket_count > 1,
        )?);
    }
    Ok(sockets)
}

fn bind_std_udp_sockets_with_reuse_port(
    listen_addr: SocketAddr,
    count: usize,
    reuse_port: bool,
) -> Result<Vec<Arc<StdUdpSocket>>> {
    let count = count.max(1);
    (0..count).map(|_| bind_std_udp_socket(listen_addr, reuse_port).map(Arc::new)).collect()
}

pub(crate) fn bind_std_udp_socket(
    listen_addr: SocketAddr,
    reuse_port: bool,
) -> Result<StdUdpSocket> {
    let socket = Socket::new(Domain::for_address(listen_addr), Type::DGRAM, Some(Protocol::UDP))
        .map_err(Error::Io)?;
    socket.set_reuse_address(true).map_err(Error::Io)?;
    #[cfg(target_os = "linux")]
    if reuse_port {
        let enabled: libc::c_int = 1;
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_REUSEPORT,
                (&enabled as *const libc::c_int).cast(),
                std::mem::size_of_val(&enabled) as libc::socklen_t,
            )
        };
        if result != 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
    }
    socket.bind(&listen_addr.into()).map_err(Error::Io)?;
    let socket: StdUdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    Ok(socket)
}
