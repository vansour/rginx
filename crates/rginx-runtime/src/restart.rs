use std::collections::HashMap;
use std::env;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{Error, Listener, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const INHERITED_LISTENERS_ENV: &str = "RGINX_INHERITED_LISTENERS";
const READY_FD_ENV: &str = "RGINX_RESTART_READY_FD";
const READY_MESSAGE: &[u8] = b"READY\n";
const CHILD_READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct ListenerHandle {
    pub listener: Listener,
    pub std_listener: Arc<StdTcpListener>,
    pub std_udp_sockets: Vec<Arc<StdUdpSocket>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InheritedSocketKind {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InheritedListenerFd {
    kind: InheritedSocketKind,
    listen_addr: SocketAddr,
    fd: RawFd,
}

pub struct InheritedListeners {
    pub tcp: HashMap<SocketAddr, StdTcpListener>,
    pub udp: HashMap<SocketAddr, Vec<StdUdpSocket>>,
}

pub async fn restart(config_path: &Path, listener_handles: &[ListenerHandle]) -> Result<()> {
    let executable = env::current_exe().map_err(Error::Io)?;
    let mut inherited = Vec::new();
    for handle in listener_handles {
        set_fd_inheritable(handle.std_listener.as_raw_fd())?;
        inherited.push(InheritedListenerFd {
            kind: InheritedSocketKind::Tcp,
            listen_addr: handle.listener.server.listen_addr,
            fd: handle.std_listener.as_raw_fd(),
        });

        if let Some(http3) = &handle.listener.http3 {
            for std_udp_socket in &handle.std_udp_sockets {
                set_fd_inheritable(std_udp_socket.as_raw_fd())?;
                inherited.push(InheritedListenerFd {
                    kind: InheritedSocketKind::Udp,
                    listen_addr: http3.listen_addr,
                    fd: std_udp_socket.as_raw_fd(),
                });
            }
        }
    }

    let (ready_parent, ready_child) = StdUnixStream::pair().map_err(Error::Io)?;
    set_fd_inheritable(ready_child.as_raw_fd())?;
    let inherited_json = serde_json::to_string(&inherited)
        .map_err(|error| Error::Server(format!("failed to encode inherited listeners: {error}")))?;

    let mut command = Command::new(executable);
    command
        .arg("--config")
        .arg(config_path)
        .env(INHERITED_LISTENERS_ENV, inherited_json)
        .env(READY_FD_ENV, ready_child.as_raw_fd().to_string())
        .stdin(Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|error| Error::Server(format!("failed to spawn replacement process: {error}")))?;
    drop(ready_child);

    ready_parent.set_nonblocking(true).map_err(Error::Io)?;
    let ready_parent = tokio::net::UnixStream::from_std(ready_parent).map_err(Error::Io)?;
    let mut ready_parent = ready_parent;
    let mut buffer = Vec::new();

    match tokio::time::timeout(CHILD_READY_TIMEOUT, ready_parent.read_to_end(&mut buffer)).await {
        Ok(Ok(_)) if buffer == READY_MESSAGE => Ok(()),
        Ok(Ok(_)) => {
            let _ = child.try_wait();
            Err(Error::Server(format!(
                "replacement process sent an unexpected readiness payload: {:?}",
                String::from_utf8_lossy(&buffer)
            )))
        }
        Ok(Err(error)) => {
            let _ = child.try_wait();
            Err(Error::Server(format!(
                "failed while waiting for replacement process readiness: {error}"
            )))
        }
        Err(_) => {
            let _ = child.start_kill();
            Err(Error::Server(format!(
                "replacement process did not become ready within {} ms",
                CHILD_READY_TIMEOUT.as_millis()
            )))
        }
    }
}

pub fn take_inherited_listeners_from_env() -> Result<InheritedListeners> {
    let Some(raw) = env::var_os(INHERITED_LISTENERS_ENV) else {
        return Ok(InheritedListeners { tcp: HashMap::new(), udp: HashMap::new() });
    };
    let raw = raw
        .into_string()
        .map_err(|_| Error::Server(format!("{INHERITED_LISTENERS_ENV} is not valid UTF-8")))?;
    let inherited = serde_json::from_str::<Vec<InheritedListenerFd>>(&raw)
        .map_err(|error| Error::Server(format!("failed to decode inherited listeners: {error}")))?;

    let mut tcp = HashMap::new();
    let mut udp = HashMap::new();
    for entry in inherited {
        match entry.kind {
            InheritedSocketKind::Tcp => {
                let listener = unsafe { StdTcpListener::from_raw_fd(entry.fd) };
                listener.set_nonblocking(true)?;
                tcp.insert(entry.listen_addr, listener);
            }
            InheritedSocketKind::Udp => {
                let socket = unsafe { StdUdpSocket::from_raw_fd(entry.fd) };
                socket.set_nonblocking(true)?;
                udp.entry(entry.listen_addr).or_insert_with(Vec::new).push(socket);
            }
        }
    }

    unsafe {
        env::remove_var(INHERITED_LISTENERS_ENV);
    }
    Ok(InheritedListeners { tcp, udp })
}

pub fn notify_ready_if_requested() -> Result<()> {
    let Some(raw_fd) = env::var_os(READY_FD_ENV) else {
        return Ok(());
    };
    let raw_fd = raw_fd
        .into_string()
        .map_err(|_| Error::Server(format!("{READY_FD_ENV} is not valid UTF-8")))?;
    let fd = raw_fd
        .parse::<RawFd>()
        .map_err(|error| Error::Server(format!("failed to parse {READY_FD_ENV}: {error}")))?;

    let mut stream = unsafe { StdUnixStream::from_raw_fd(fd) };
    stream.write_all(READY_MESSAGE).map_err(Error::Io)?;
    stream.flush().map_err(Error::Io)?;
    unsafe {
        env::remove_var(READY_FD_ENV);
    }
    Ok(())
}

fn set_fd_inheritable(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(Error::Io(io::Error::last_os_error()));
    }

    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    if result < 0 {
        return Err(Error::Io(io::Error::last_os_error()));
    }

    Ok(())
}

#[cfg(test)]
mod tests;
