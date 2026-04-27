use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rginx_core::{Result, Upstream, VirtualHost};

use crate::model::VirtualHostConfig;

pub(super) struct CompiledVirtualHost {
    pub(super) vhost: VirtualHost,
    pub(super) upstreams: HashMap<String, Arc<Upstream>>,
}

pub(super) fn compile_virtual_host(
    vhost_id: String,
    config: VirtualHostConfig,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
) -> Result<CompiledVirtualHost> {
    let VirtualHostConfig {
        listen: _,
        server_names,
        upstreams: raw_upstreams,
        locations,
        tls,
        http3: _,
    } = config;
    let local_upstream_names = raw_upstreams
        .iter()
        .map(|upstream| {
            (
                upstream.name.clone(),
                super::upstream::scoped_upstream_name(&vhost_id, &upstream.name),
            )
        })
        .collect::<HashMap<_, _>>();
    let local_upstreams =
        super::upstream::compile_scoped_upstreams(raw_upstreams, base_dir, &vhost_id)?;
    let mut visible_upstreams = upstreams.clone();
    visible_upstreams.extend(
        local_upstreams.iter().map(|(name, upstream)| (name.clone(), Arc::clone(upstream))),
    );

    let routes = super::route::compile_routes_with_local(
        locations,
        &visible_upstreams,
        &local_upstream_names,
        &vhost_id,
    )?;
    let tls = super::server::compile_virtual_host_tls(tls, base_dir)?;

    Ok(CompiledVirtualHost {
        vhost: VirtualHost { id: vhost_id, server_names, routes, tls },
        upstreams: local_upstreams,
    })
}
