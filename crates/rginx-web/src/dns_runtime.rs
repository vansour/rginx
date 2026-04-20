use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rginx_control_store::ControlPlaneStore;
use rginx_control_types::{DnsPublishedSnapshot, DnsRecordType, DnsRuntimeStatus};
use rginx_dns::{DnsQueryEngine, DnsQueryResult, DnsServerConfig, InMemoryDnsRuntime};
use tokio::sync::watch;

#[derive(Debug)]
pub struct DnsRuntimeManager {
    store: ControlPlaneStore,
    runtime: InMemoryDnsRuntime,
}

impl DnsRuntimeManager {
    pub fn new(
        store: ControlPlaneStore,
        udp_bind_addr: Option<SocketAddr>,
        tcp_bind_addr: Option<SocketAddr>,
    ) -> Self {
        Self { store, runtime: InMemoryDnsRuntime::new(udp_bind_addr, tcp_bind_addr) }
    }

    pub fn server_config(&self) -> DnsServerConfig {
        self.runtime.server_config()
    }

    pub fn enabled(&self) -> bool {
        self.runtime.enabled()
    }

    pub async fn refresh(&self) -> Result<()> {
        let snapshots = load_published_snapshots(&self.store).await?;
        self.runtime.replace_snapshots(snapshots);
        Ok(())
    }

    pub async fn run_refresh_loop(
        self: Arc<Self>,
        interval: Duration,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(error) = self.refresh().await {
                        tracing::warn!(error = %error, "dns runtime refresh failed");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn runtime_status(&self) -> Vec<DnsRuntimeStatus> {
        self.runtime.runtime_status()
    }
}

impl DnsQueryEngine for DnsRuntimeManager {
    fn resolve(
        &self,
        qname: &str,
        record_type: DnsRecordType,
        source_ip: IpAddr,
    ) -> DnsQueryResult {
        self.runtime.resolve(qname, record_type, source_ip)
    }
}

pub async fn load_published_snapshots(
    store: &ControlPlaneStore,
) -> Result<Vec<DnsPublishedSnapshot>> {
    let published = store.dns_repository().load_all_published_revisions().await?;
    let nodes = store.node_repository().list_nodes().await?;
    let mut snapshots = Vec::with_capacity(published.len());

    for revision in published {
        snapshots.push(DnsPublishedSnapshot {
            cluster_id: revision.cluster_id.clone(),
            revision_id: revision.revision_id.clone(),
            version_label: revision.version_label.clone(),
            plan: revision.plan,
            nodes: nodes.clone(),
            resolved_upstreams: load_resolved_upstreams(store, &revision.cluster_id)
                .await
                .unwrap_or_default(),
        });
    }

    Ok(snapshots)
}

async fn load_resolved_upstreams(
    store: &ControlPlaneStore,
    cluster_id: &str,
) -> Result<BTreeMap<String, Vec<String>>> {
    let Some(revision) =
        store.revision_repository().load_latest_revision_for_cluster(cluster_id).await?
    else {
        return Ok(BTreeMap::new());
    };
    let Some(detail) =
        store.revision_repository().load_revision_detail(&revision.revision_id).await?
    else {
        return Ok(BTreeMap::new());
    };
    let compiled = match rginx_config::load_and_compile_from_str(
        &detail.config_text,
        Path::new(&detail.source_path),
    ) {
        Ok(compiled) => compiled,
        Err(error) => {
            tracing::warn!(
                cluster_id = %cluster_id,
                revision_id = %detail.revision_id,
                error = %error,
                "failed to compile latest config revision while refreshing dns runtime upstream targets"
            );
            return Ok(BTreeMap::new());
        }
    };

    let mut output = BTreeMap::new();
    for (upstream_name, upstream) in compiled.upstreams {
        let mut addrs = Vec::new();
        for peer in &upstream.peers {
            if let Ok(resolved) = peer.authority.to_socket_addrs() {
                addrs.extend(resolved.map(|addr: SocketAddr| addr.ip().to_string()));
            }
        }
        addrs.sort();
        addrs.dedup();
        output.insert(upstream_name, addrs);
    }
    Ok(output)
}
