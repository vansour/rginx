use rginx_core::Result;

use crate::model::{ListenerConfig, ServerConfig, VirtualHostConfig};

mod base;
mod listeners;

pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    base::validate_server(server)
}

pub(super) fn validate_listeners(
    listeners: &[ListenerConfig],
    server: &ServerConfig,
    vhosts: &[VirtualHostConfig],
) -> Result<()> {
    listeners::validate_listeners(listeners, server, vhosts)
}
