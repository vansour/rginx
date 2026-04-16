create table if not exists cp_clusters (
    cluster_id text primary key,
    display_name text not null,
    created_at timestamptz not null default now()
);

create table if not exists cp_nodes (
    node_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    advertise_addr text not null,
    role text not null,
    state text not null,
    running_version text not null,
    last_seen_at timestamptz,
    created_at timestamptz not null default now()
);

create table if not exists cp_config_revisions (
    revision_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    version_label text not null,
    summary text not null default '',
    rendered_config jsonb not null,
    created_at timestamptz not null default now()
);

create table if not exists cp_deployments (
    deployment_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    revision_id text not null references cp_config_revisions (revision_id) on delete restrict,
    status text not null,
    target_nodes integer not null default 0,
    healthy_nodes integer not null default 0,
    created_at timestamptz not null default now(),
    finished_at timestamptz
);

create index if not exists cp_nodes_cluster_id_idx on cp_nodes (cluster_id);
create index if not exists cp_nodes_last_seen_at_idx on cp_nodes (last_seen_at desc nulls last);
create index if not exists cp_config_revisions_cluster_id_idx on cp_config_revisions (cluster_id);
create index if not exists cp_deployments_cluster_id_idx on cp_deployments (cluster_id);
create index if not exists cp_deployments_status_idx on cp_deployments (status);
