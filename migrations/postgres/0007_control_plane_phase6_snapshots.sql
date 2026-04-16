create table if not exists cp_node_snapshots (
    snapshot_id bigserial primary key,
    node_id text not null references cp_nodes (node_id) on delete cascade,
    snapshot_version bigint not null,
    schema_version integer not null,
    captured_at timestamptz not null,
    pid integer not null,
    binary_version text not null,
    included_modules jsonb not null default '[]'::jsonb,
    status jsonb,
    counters jsonb,
    traffic jsonb,
    peer_health jsonb,
    upstreams jsonb,
    payload jsonb not null default '{}'::jsonb,
    created_at timestamptz not null default now(),
    unique (node_id, snapshot_version)
);

create index if not exists cp_node_snapshots_node_id_idx
    on cp_node_snapshots (node_id);

create index if not exists cp_node_snapshots_captured_at_idx
    on cp_node_snapshots (captured_at desc);
