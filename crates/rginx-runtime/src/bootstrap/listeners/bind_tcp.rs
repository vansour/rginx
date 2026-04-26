use std::net::TcpListener as StdTcpListener;

use rginx_core::Result;

pub(crate) fn bind_std_listener(listen_addr: std::net::SocketAddr) -> Result<StdTcpListener> {
    let socket = StdTcpListener::bind(listen_addr)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
