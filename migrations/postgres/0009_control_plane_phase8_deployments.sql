alter table cp_deployments
    add column if not exists created_by text not null default 'system',
    add column if not exists parallelism integer not null default 1,
    add column if not exists failure_threshold integer not null default 1,
    add column if not exists auto_rollback boolean not null default false,
    add column if not exists rollback_of_deployment_id text references cp_deployments (deployment_id) on delete set null,
    add column if not exists rollback_revision_id text references cp_config_revisions (revision_id) on delete set null,
    add column if not exists status_reason text,
    add column if not exists idempotency_key text,
    add column if not exists started_at timestamptz,
    add column if not exists updated_at timestamptz not null default now();

update cp_deployments
set created_by = started_by
where created_by = 'system'
  and started_by <> 'system';

update cp_deployments
set started_at = coalesce(started_at, created_at),
    updated_at = coalesce(updated_at, created_at)
where started_at is null
   or updated_at is null;

create unique index if not exists cp_deployments_idempotency_key_idx
    on cp_deployments (idempotency_key)
    where idempotency_key is not null;

create index if not exists cp_deployments_created_at_idx
    on cp_deployments (created_at desc);

create index if not exists cp_deployments_rollback_of_idx
    on cp_deployments (rollback_of_deployment_id);

create table if not exists cp_deployment_targets (
    target_id text primary key,
    deployment_id text not null references cp_deployments (deployment_id) on delete cascade,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    node_id text not null references cp_nodes (node_id) on delete cascade,
    desired_revision_id text not null references cp_config_revisions (revision_id) on delete restrict,
    state text not null,
    task_id text,
    attempt_count integer not null default 0,
    batch_index integer not null default 0,
    last_error text,
    dispatched_at timestamptz,
    acked_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (deployment_id, node_id)
);

create index if not exists cp_deployment_targets_deployment_id_idx
    on cp_deployment_targets (deployment_id);

create index if not exists cp_deployment_targets_deployment_state_idx
    on cp_deployment_targets (deployment_id, state);

create index if not exists cp_deployment_targets_node_id_idx
    on cp_deployment_targets (node_id);

create table if not exists cp_agent_tasks (
    task_id text primary key,
    deployment_id text not null references cp_deployments (deployment_id) on delete cascade,
    target_id text not null unique references cp_deployment_targets (target_id) on delete cascade,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    node_id text not null references cp_nodes (node_id) on delete cascade,
    kind text not null,
    state text not null,
    revision_id text not null references cp_config_revisions (revision_id) on delete restrict,
    source_path text not null,
    config_text text not null,
    attempt integer not null default 1,
    completion_idempotency_key text,
    result_message text,
    result_payload jsonb not null default '{}'::jsonb,
    dispatched_at timestamptz not null default now(),
    acked_at timestamptz,
    completed_at timestamptz,
    updated_at timestamptz not null default now()
);

create index if not exists cp_agent_tasks_node_state_idx
    on cp_agent_tasks (node_id, state, dispatched_at asc);

create index if not exists cp_agent_tasks_deployment_id_idx
    on cp_agent_tasks (deployment_id);

create index if not exists cp_agent_tasks_target_id_idx
    on cp_agent_tasks (target_id);
