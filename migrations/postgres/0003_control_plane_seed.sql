insert into cp_clusters (cluster_id, display_name)
values ('cluster-mainland', 'Mainland Edge Cluster')
on conflict (cluster_id) do nothing;

insert into cp_config_revisions (
    revision_id,
    cluster_id,
    version_label,
    summary,
    rendered_config,
    created_by,
    created_at
)
values (
    'rev_local_0001',
    'cluster-mainland',
    'v0.1.3-rc.11',
    'seeded control-plane revision for docker bootstrap',
    jsonb_build_object(
        'entrypoint', 'configs/rginx.ron',
        'runtime', jsonb_build_object('worker_threads', 2, 'accept_workers', 1)
    ),
    'system:seed',
    now() - interval '15 minutes'
)
on conflict (revision_id) do nothing;

insert into cp_nodes (
    node_id,
    cluster_id,
    advertise_addr,
    role,
    state,
    running_version,
    last_seen_at,
    created_at
)
values
    (
        'edge-sha-01',
        'cluster-mainland',
        '10.0.0.11:8443',
        'edge',
        'online',
        'v0.1.3-rc.11',
        now() - interval '30 seconds',
        now() - interval '45 days'
    ),
    (
        'edge-sz-01',
        'cluster-mainland',
        '10.0.1.21:8443',
        'edge',
        'draining',
        'v0.1.3-rc.11',
        now() - interval '90 seconds',
        now() - interval '30 days'
    )
on conflict (node_id) do nothing;

insert into cp_deployments (
    deployment_id,
    cluster_id,
    revision_id,
    status,
    target_nodes,
    healthy_nodes,
    started_by,
    created_at
)
values (
    'deploy_local_0001',
    'cluster-mainland',
    'rev_local_0001',
    'running',
    2,
    1,
    'system:seed',
    now() - interval '5 minutes'
)
on conflict (deployment_id) do nothing;

insert into cp_node_heartbeats (
    node_id,
    admin_socket_path,
    state,
    observed_at,
    payload
)
values
    (
        'edge-sha-01',
        '/run/rginx/admin.sock',
        'online',
        '2026-04-16T08:59:30Z'::timestamptz,
        jsonb_build_object('snapshot_version', 11, 'tls_ready', true)
    ),
    (
        'edge-sz-01',
        '/run/rginx/admin.sock',
        'draining',
        '2026-04-16T08:58:30Z'::timestamptz,
        jsonb_build_object('snapshot_version', 9, 'tls_ready', true)
    )
on conflict do nothing;

insert into cp_audit_logs (
    audit_id,
    cluster_id,
    actor_id,
    action,
    resource_type,
    resource_id,
    result,
    details,
    created_at
)
values
    (
        'audit_local_0001',
        'cluster-mainland',
        'system:seed',
        'revision.seeded',
        'config_revision',
        'rev_local_0001',
        'succeeded',
        jsonb_build_object('source', 'migrations', 'version_label', 'v0.1.3-rc.11'),
        now() - interval '15 minutes'
    ),
    (
        'audit_local_0002',
        'cluster-mainland',
        'system:seed',
        'deployment.started',
        'deployment',
        'deploy_local_0001',
        'running',
        jsonb_build_object('target_nodes', 2, 'healthy_nodes', 1),
        now() - interval '5 minutes'
    )
on conflict (audit_id) do nothing;
