alter table cp_nodes
    add column if not exists admin_socket_path text not null default '/run/rginx/admin.sock',
    add column if not exists last_snapshot_version bigint,
    add column if not exists runtime_revision bigint,
    add column if not exists runtime_pid integer,
    add column if not exists listener_count integer,
    add column if not exists active_connections integer,
    add column if not exists status_reason text,
    add column if not exists updated_at timestamptz not null default now();

update cp_nodes as n
set admin_socket_path = hb.admin_socket_path,
    last_snapshot_version = coalesce(
        case
            when (hb.payload ->> 'snapshot_version') ~ '^[0-9]+$'
                then (hb.payload ->> 'snapshot_version')::bigint
            else null
        end,
        n.last_snapshot_version
    ),
    updated_at = now()
from (
    select distinct on (node_id)
        node_id,
        admin_socket_path,
        payload
    from cp_node_heartbeats
    order by node_id, observed_at desc
) as hb
where hb.node_id = n.node_id;

create index if not exists cp_nodes_state_idx on cp_nodes (state);
create index if not exists cp_nodes_updated_at_idx on cp_nodes (updated_at desc);
