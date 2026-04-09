use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rginx_core::{Result, Upstream, VirtualHost};

use crate::model::VirtualHostConfig;

pub(super) fn compile_virtual_host(
    vhost_id: String,
    config: VirtualHostConfig,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
) -> Result<VirtualHost> {
    let VirtualHostConfig { server_names, locations, tls } = config;
    let routes = super::route::compile_routes(locations, upstreams, &vhost_id)?;
    let tls = super::server::compile_virtual_host_tls(tls, base_dir)?;

    Ok(VirtualHost { id: vhost_id, server_names, routes, tls })
}
