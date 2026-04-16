alter table cp_config_revisions
    add column if not exists created_by text not null default 'system';

alter table cp_deployments
    add column if not exists started_by text not null default 'system';

create table if not exists cp_node_heartbeats (
    heartbeat_id bigserial primary key,
    node_id text not null references cp_nodes (node_id) on delete cascade,
    admin_socket_path text not null,
    state text not null,
    observed_at timestamptz not null,
    payload jsonb not null default '{}'::jsonb
);

create index if not exists cp_node_heartbeats_node_id_idx on cp_node_heartbeats (node_id);
create index if not exists cp_node_heartbeats_observed_at_idx on cp_node_heartbeats (observed_at desc);
create unique index if not exists cp_node_heartbeats_node_observed_idx
    on cp_node_heartbeats (node_id, observed_at);

create table if not exists cp_audit_logs (
    audit_id text primary key,
    cluster_id text references cp_clusters (cluster_id) on delete set null,
    actor_id text not null,
    action text not null,
    resource_type text not null,
    resource_id text not null,
    result text not null,
    details jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now()
);

create index if not exists cp_audit_logs_cluster_id_idx on cp_audit_logs (cluster_id);
create index if not exists cp_audit_logs_resource_idx on cp_audit_logs (resource_type, resource_id);
create index if not exists cp_audit_logs_created_at_idx on cp_audit_logs (created_at desc);
