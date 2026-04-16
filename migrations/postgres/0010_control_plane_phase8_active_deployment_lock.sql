create unique index if not exists cp_deployments_one_active_per_cluster_idx
    on cp_deployments (cluster_id)
    where status in ('running', 'paused');
