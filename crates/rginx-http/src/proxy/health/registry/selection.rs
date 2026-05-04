use super::*;

impl PeerHealthRegistry {
    pub(crate) async fn select_peers(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
    ) -> SelectedPeers {
        if upstream.load_balance == UpstreamLoadBalance::LeastConn {
            return self.select_peers_by_least_conn(client, upstream, limit).await;
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_in_pool(client, upstream, client_ip, limit, true).await;
        }

        let primary = self.select_peers_in_pool(client, upstream, client_ip, limit, false).await;
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_in_pool(client, upstream, client_ip, limit, true).await,
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_in_pool(client, upstream, client_ip, remaining, true).await,
        )
    }

    async fn select_peers_by_least_conn(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        if !upstream.has_primary_peers() {
            return self.select_peers_by_least_conn_in_pool(client, upstream, limit, true).await;
        }

        let primary = self.select_peers_by_least_conn_in_pool(client, upstream, limit, false).await;
        if primary.peers.is_empty() {
            return merge_selected_peers(
                primary,
                self.select_peers_by_least_conn_in_pool(client, upstream, limit, true).await,
            );
        }

        if primary.peers.len() == limit {
            return primary;
        }

        let remaining = limit - primary.peers.len();
        merge_selected_peers(
            primary,
            self.select_peers_by_least_conn_in_pool(client, upstream, remaining, true).await,
        )
    }

    async fn select_peers_in_pool(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        client_ip: std::net::IpAddr,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let ordered = if backup {
            upstream.backup_peers_for_client_ip(client_ip, upstream.peers.len())
        } else {
            upstream.primary_peers_for_client_ip(client_ip, upstream.peers.len())
        };

        self.select_available_peers(client, upstream, ordered, limit).await
    }

    async fn select_peers_by_least_conn_in_pool(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        limit: usize,
        backup: bool,
    ) -> SelectedPeers {
        let mut available = Vec::new();
        let mut skipped_unhealthy = 0;

        for (order, peer) in upstream.peers.iter().cloned().enumerate() {
            if peer.backup != backup {
                continue;
            }

            let endpoints: Vec<ResolvedUpstreamPeer> =
                client.resolve_peer(&peer).await.unwrap_or_default();
            for endpoint in endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
                let active_requests =
                    self.active_requests(&upstream.name, &endpoint.logical_peer_url);
                if self.endpoint_is_selectable(&upstream.name, &endpoint, active_requests) {
                    available.push((active_requests, order, endpoint));
                } else {
                    skipped_unhealthy += 1;
                }
            }
        }

        available.sort_by(|left, right| {
            projected_least_conn_load(left.0, left.2.weight, right.0, right.2.weight)
                .then(right.2.weight.cmp(&left.2.weight))
                .then(left.1.cmp(&right.1))
                .then(left.2.dial_authority.cmp(&right.2.dial_authority))
        });

        SelectedPeers {
            peers: available.into_iter().take(limit).map(|(_, _, peer)| peer).collect(),
            skipped_unhealthy,
        }
    }

    async fn select_available_peers(
        &self,
        client: &ProxyClient,
        upstream: &Upstream,
        ordered: Vec<UpstreamPeer>,
        limit: usize,
    ) -> SelectedPeers {
        if limit == 0 {
            return SelectedPeers { peers: Vec::new(), skipped_unhealthy: 0 };
        }

        let mut batches = Vec::new();
        let mut skipped_unhealthy = 0;

        for peer in ordered {
            let mut endpoints: Vec<ResolvedUpstreamPeer> =
                client.resolve_peer(&peer).await.unwrap_or_default();
            for endpoint in &endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
            }
            endpoints.sort_by(|left, right| {
                self.active_requests(&upstream.name, &left.logical_peer_url)
                    .cmp(&self.active_requests(&upstream.name, &right.logical_peer_url))
                    .then(left.dial_authority.cmp(&right.dial_authority))
            });

            let mut available = Vec::new();
            for endpoint in endpoints {
                self.ensure_endpoint(
                    &upstream.name,
                    &endpoint.endpoint_key,
                    &endpoint.logical_peer_url,
                );
                let active_requests =
                    self.active_requests(&upstream.name, &endpoint.logical_peer_url);
                if self.endpoint_is_selectable(&upstream.name, &endpoint, active_requests) {
                    available.push(endpoint);
                } else {
                    skipped_unhealthy += 1;
                }
            }
            if !available.is_empty() {
                batches.push(available);
            }
        }

        let mut selected = Vec::new();
        let mut depth = 0usize;
        while selected.len() < limit {
            let mut advanced = false;
            for batch in &batches {
                if let Some(endpoint) = batch.get(depth) {
                    selected.push(endpoint.clone());
                    advanced = true;
                    if selected.len() == limit {
                        break;
                    }
                }
            }
            if !advanced {
                break;
            }
            depth += 1;
        }

        SelectedPeers { peers: selected, skipped_unhealthy }
    }

    fn endpoint_is_selectable(
        &self,
        upstream_name: &str,
        endpoint: &ResolvedUpstreamPeer,
        active_requests: u64,
    ) -> bool {
        if !self.is_available(upstream_name, &endpoint.endpoint_key) {
            return false;
        }

        endpoint.max_conns.is_none_or(|max_conns| active_requests < max_conns as u64)
    }
}

fn merge_selected_peers(mut primary: SelectedPeers, secondary: SelectedPeers) -> SelectedPeers {
    primary.skipped_unhealthy += secondary.skipped_unhealthy;
    primary.peers.extend(secondary.peers);
    primary
}

fn projected_least_conn_load(
    left_active_requests: u64,
    left_weight: u32,
    right_active_requests: u64,
    right_weight: u32,
) -> std::cmp::Ordering {
    let left = u128::from(left_active_requests.saturating_add(1)) * u128::from(right_weight.max(1));
    let right =
        u128::from(right_active_requests.saturating_add(1)) * u128::from(left_weight.max(1));
    left.cmp(&right)
}
