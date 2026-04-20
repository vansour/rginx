use std::num::NonZeroU32;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::{config::BootstrapAdminConfig, repositories::ControlPlaneStore};

const BOOTSTRAP_LOCK_KEY: i64 = 5_233_469_927_313_561_198;
const BOOTSTRAP_ADMIN_USER_ID: &str = "usr_local_admin";
const BOOTSTRAP_ADMIN_ROLE_ID: &str = "super_admin";
const BOOTSTRAP_ADMIN_SEED_AUDIT_ID: &str = "audit_auth_seed_0001";
const BOOTSTRAP_ADMIN_SEED_REQUEST_ID: &str = "seed-auth-0001";
const PASSWORD_SCHEME: &str = "pbkdf2_sha256";
const PASSWORD_ITERATIONS: u32 = 100_000;
const PASSWORD_HASH_LEN: usize = 32;
const PASSWORD_SALT_LEN: usize = 16;

const BOOTSTRAP_STEPS: &[(&str, &str)] = &[
    (
        "0001_control_plane_bootstrap",
        r#"
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
"#,
    ),
    (
        "0002_control_plane_phase3_schema",
        r#"
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
"#,
    ),
    (
        "0003_control_plane_seed",
        r#"
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
"#,
    ),
    (
        "0004_control_plane_auth_schema",
        r#"
create table if not exists cp_users (
    user_id text primary key,
    username text not null unique,
    display_name text not null,
    password_hash text not null,
    is_active boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists cp_roles (
    role_id text primary key,
    display_name text not null,
    description text not null default '',
    created_at timestamptz not null default now()
);

create table if not exists cp_user_roles (
    user_id text not null references cp_users (user_id) on delete cascade,
    role_id text not null references cp_roles (role_id) on delete cascade,
    assigned_at timestamptz not null default now(),
    primary key (user_id, role_id)
);

create table if not exists cp_api_sessions (
    session_id text primary key,
    user_id text not null references cp_users (user_id) on delete cascade,
    session_hash text not null unique,
    issued_at timestamptz not null default now(),
    expires_at timestamptz not null,
    revoked_at timestamptz,
    last_seen_at timestamptz not null default now(),
    user_agent text,
    remote_addr text
);

create index if not exists cp_api_sessions_user_id_idx on cp_api_sessions (user_id);
create index if not exists cp_api_sessions_expires_at_idx on cp_api_sessions (expires_at);
create index if not exists cp_api_sessions_last_seen_at_idx on cp_api_sessions (last_seen_at desc);

alter table cp_audit_logs
    add column if not exists request_id text not null default 'unknown';

create index if not exists cp_audit_logs_request_id_idx on cp_audit_logs (request_id);
"#,
    ),
    (
        "0005_control_plane_auth_seed",
        r#"
insert into cp_roles (role_id, display_name, description)
values
    ('super_admin', 'Admin', 'single control-plane administrator access')
on conflict (role_id) do nothing;

insert into cp_users (
    user_id,
    username,
    display_name,
    password_hash,
    is_active,
    created_at,
    updated_at
)
values
    (
        'usr_local_admin',
        'admin',
        'Local Admin',
        'pbkdf2_sha256$100000$vWN8LkG6Fsf9o-nQJ9s77g$ipdvRiVMvC5zZw8GMuowvR6rcCsTlpXxC7jZ3JsPxjM',
        true,
        now() - interval '7 days',
        now() - interval '7 days'
    )
on conflict (user_id) do nothing;

insert into cp_user_roles (user_id, role_id)
values
    ('usr_local_admin', 'super_admin')
on conflict (user_id, role_id) do nothing;

insert into cp_audit_logs (
    audit_id,
    request_id,
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
        'audit_auth_seed_0001',
        'seed-auth-0001',
        null,
        'system:seed',
        'auth.bootstrap_seeded',
        'user',
        'usr_local_admin',
        'succeeded',
        jsonb_build_object('username', 'admin', 'role', 'super_admin'),
        now() - interval '7 days'
    )
on conflict (audit_id) do nothing;
"#,
    ),
    (
        "0006_control_plane_phase5_nodes",
        r#"
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
"#,
    ),
    (
        "0007_control_plane_phase6_snapshots",
        r#"
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
"#,
    ),
    (
        "0008_control_plane_phase7_revisions",
        r#"
alter table cp_config_revisions
    add column if not exists source_path text not null default 'configs/rginx.ron';

alter table cp_config_revisions
    add column if not exists config_text text not null default '';

alter table cp_config_revisions
    add column if not exists compile_summary jsonb not null default '{}'::jsonb;

update cp_config_revisions
set config_text = $phase7_seed_revision$
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
        worker_threads: Some(2),
        accept_workers: Some(1),
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
        server_names: ["localhost"],
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\n"),
            ),
        ),
    ],
)
$phase7_seed_revision$
where config_text = '';

create table if not exists cp_config_drafts (
    draft_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    title text not null,
    summary text not null default '',
    source_path text not null,
    config_text text not null,
    base_revision_id text references cp_config_revisions (revision_id) on delete set null,
    validation_state text not null,
    validation_errors jsonb not null default '[]'::jsonb,
    compile_summary jsonb not null default '{}'::jsonb,
    validated_at timestamptz,
    published_revision_id text references cp_config_revisions (revision_id) on delete set null,
    created_by text not null,
    updated_by text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index if not exists cp_config_drafts_cluster_id_idx on cp_config_drafts (cluster_id);
create index if not exists cp_config_drafts_updated_at_idx on cp_config_drafts (updated_at desc);
"#,
    ),
    (
        "0009_control_plane_phase8_deployments",
        r#"
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
"#,
    ),
    (
        "0010_control_plane_phase8_active_deployment_lock",
        r#"
create unique index if not exists cp_deployments_one_active_per_cluster_idx
    on cp_deployments (cluster_id)
    where status in ('running', 'paused');
"#,
    ),
    (
        "0011_control_plane_dns_schema",
        r#"
create table if not exists cp_dns_revisions (
    revision_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    version_label text not null,
    summary text not null default '',
    plan_json jsonb not null,
    validation_summary jsonb not null,
    created_by text not null,
    created_at timestamptz not null default now(),
    published_at timestamptz
);

create table if not exists cp_dns_drafts (
    draft_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    title text not null,
    summary text not null default '',
    plan_json jsonb not null,
    base_revision_id text references cp_dns_revisions (revision_id) on delete set null,
    validation_state text not null default 'pending',
    validation_summary jsonb,
    validated_at timestamptz,
    published_revision_id text references cp_dns_revisions (revision_id) on delete set null,
    created_by text not null,
    updated_by text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists cp_dns_runtime_state (
    cluster_id text primary key references cp_clusters (cluster_id) on delete cascade,
    published_revision_id text references cp_dns_revisions (revision_id) on delete set null,
    updated_at timestamptz not null default now()
);

create index if not exists cp_dns_revisions_cluster_id_idx
    on cp_dns_revisions (cluster_id, created_at desc);
create unique index if not exists cp_dns_revisions_cluster_version_label_idx
    on cp_dns_revisions (cluster_id, version_label);
create index if not exists cp_dns_drafts_cluster_id_idx
    on cp_dns_drafts (cluster_id, updated_at desc);
create index if not exists cp_dns_drafts_validation_state_idx
    on cp_dns_drafts (validation_state);
"#,
    ),
    (
        "0012_control_plane_dns_deployments",
        r#"
create table if not exists cp_dns_deployments (
    deployment_id text primary key,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    revision_id text not null references cp_dns_revisions (revision_id) on delete cascade,
    status text not null,
    target_nodes integer not null,
    created_by text not null,
    parallelism integer not null,
    failure_threshold integer not null,
    auto_rollback boolean not null default false,
    promotes_cluster_runtime boolean not null default false,
    rollback_of_deployment_id text references cp_dns_deployments (deployment_id) on delete set null,
    rollback_revision_id text references cp_dns_revisions (revision_id) on delete set null,
    status_reason text,
    idempotency_key text,
    created_at timestamptz not null default now(),
    started_at timestamptz,
    finished_at timestamptz,
    updated_at timestamptz not null default now()
);

create table if not exists cp_dns_deployment_targets (
    target_id text primary key,
    deployment_id text not null references cp_dns_deployments (deployment_id) on delete cascade,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    node_id text not null references cp_nodes (node_id) on delete cascade,
    desired_revision_id text not null references cp_dns_revisions (revision_id) on delete cascade,
    state text not null,
    batch_index integer not null default 0,
    last_error text,
    assigned_at timestamptz,
    confirmed_at timestamptz,
    failed_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists cp_dns_node_overrides (
    node_id text primary key references cp_nodes (node_id) on delete cascade,
    cluster_id text not null references cp_clusters (cluster_id) on delete cascade,
    published_revision_id text references cp_dns_revisions (revision_id) on delete set null,
    deployment_id text references cp_dns_deployments (deployment_id) on delete set null,
    updated_at timestamptz not null default now()
);

create unique index if not exists cp_dns_deployments_idempotency_key_idx
    on cp_dns_deployments (idempotency_key)
    where idempotency_key is not null;
create unique index if not exists cp_dns_deployments_one_active_per_cluster_idx
    on cp_dns_deployments (cluster_id)
    where status in ('running', 'paused');
create unique index if not exists cp_dns_deployments_one_rollback_per_source_idx
    on cp_dns_deployments (rollback_of_deployment_id)
    where rollback_of_deployment_id is not null;
create index if not exists cp_dns_deployments_cluster_id_idx
    on cp_dns_deployments (cluster_id, created_at desc);
create index if not exists cp_dns_deployment_targets_deployment_id_idx
    on cp_dns_deployment_targets (deployment_id, batch_index asc, node_id asc);
create index if not exists cp_dns_deployment_targets_node_id_idx
    on cp_dns_deployment_targets (node_id);
create index if not exists cp_dns_deployment_targets_state_idx
    on cp_dns_deployment_targets (state);
create index if not exists cp_dns_node_overrides_cluster_id_idx
    on cp_dns_node_overrides (cluster_id, updated_at desc);
"#,
    ),
];

impl ControlPlaneStore {
    pub async fn bootstrap(&self) -> Result<()> {
        bootstrap_postgres(self.postgres(), &self.config().bootstrap_admin).await
    }
}

async fn bootstrap_postgres(pool: &PgPool, bootstrap_admin: &BootstrapAdminConfig) -> Result<()> {
    let mut transaction =
        pool.begin().await.context("failed to start control-plane bootstrap transaction")?;

    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(BOOTSTRAP_LOCK_KEY)
        .execute(&mut *transaction)
        .await
        .context("failed to acquire control-plane bootstrap advisory lock")?;

    for (name, sql) in BOOTSTRAP_STEPS {
        sqlx::raw_sql(sql)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("failed to apply control-plane bootstrap step `{name}`"))?;
    }

    ensure_bootstrap_admin(&mut transaction, bootstrap_admin).await?;

    transaction.commit().await.context("failed to commit control-plane bootstrap transaction")?;

    Ok(())
}

#[derive(Debug, Clone)]
struct BootstrapAdminRow {
    username: String,
    display_name: String,
    password_hash: String,
    is_active: bool,
}

async fn ensure_bootstrap_admin(
    transaction: &mut Transaction<'_, Postgres>,
    bootstrap_admin: &BootstrapAdminConfig,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into cp_roles (role_id, display_name, description)
        values ($1, 'Admin', 'single control-plane administrator access')
        on conflict (role_id) do update
        set
            display_name = excluded.display_name,
            description = excluded.description
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_ROLE_ID)
    .execute(&mut **transaction)
    .await
    .context("failed to upsert bootstrap admin role")?;

    sqlx::query(
        r#"
        delete from cp_user_roles
        where user_id <> $1 or role_id <> $2
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_USER_ID)
    .bind(BOOTSTRAP_ADMIN_ROLE_ID)
    .execute(&mut **transaction)
    .await
    .context("failed to prune non-admin role assignments")?;

    sqlx::query(
        r#"
        delete from cp_users
        where user_id <> $1
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_USER_ID)
    .execute(&mut **transaction)
    .await
    .context("failed to prune non-admin users")?;

    sqlx::query(
        r#"
        delete from cp_roles
        where role_id <> $1
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_ROLE_ID)
    .execute(&mut **transaction)
    .await
    .context("failed to prune non-admin roles")?;

    let existing = sqlx::query(
        r#"
        select username, display_name, password_hash, is_active
        from cp_users
        where user_id = $1
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_USER_ID)
    .fetch_optional(&mut **transaction)
    .await
    .context("failed to load bootstrap admin account")?
    .map(|row| {
        Ok::<_, sqlx::Error>(BootstrapAdminRow {
            username: row.try_get("username")?,
            display_name: row.try_get("display_name")?,
            password_hash: row.try_get("password_hash")?,
            is_active: row.try_get("is_active")?,
        })
    })
    .transpose()
    .context("failed to decode bootstrap admin account")?;

    let needs_password_update = match existing.as_ref() {
        Some(current) => !verify_password_hash(&current.password_hash, &bootstrap_admin.password)
            .context("failed to verify bootstrap admin password hash")?,
        None => true,
    };
    let needs_profile_update = match existing.as_ref() {
        Some(current) => {
            current.username != bootstrap_admin.username
                || current.display_name != bootstrap_admin.display_name
                || !current.is_active
        }
        None => true,
    };

    if needs_profile_update || needs_password_update {
        let password_hash = match existing.as_ref() {
            Some(current) if !needs_password_update => current.password_hash.clone(),
            _ => hash_password(&bootstrap_admin.password)
                .context("failed to hash bootstrap admin password")?,
        };

        sqlx::query(
            r#"
            insert into cp_users (
                user_id,
                username,
                display_name,
                password_hash,
                is_active,
                created_at,
                updated_at
            )
            values ($1, $2, $3, $4, true, now() - interval '7 days', now())
            on conflict (user_id) do update
            set
                username = excluded.username,
                display_name = excluded.display_name,
                password_hash = excluded.password_hash,
                is_active = excluded.is_active,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(BOOTSTRAP_ADMIN_USER_ID)
        .bind(&bootstrap_admin.username)
        .bind(&bootstrap_admin.display_name)
        .bind(&password_hash)
        .execute(&mut **transaction)
        .await
        .with_context(|| {
            format!("failed to upsert bootstrap admin account `{}`", bootstrap_admin.username)
        })?;
    }

    sqlx::query(
        r#"
        insert into cp_user_roles (user_id, role_id)
        values ($1, $2)
        on conflict (user_id, role_id) do nothing
        "#,
    )
    .bind(BOOTSTRAP_ADMIN_USER_ID)
    .bind(BOOTSTRAP_ADMIN_ROLE_ID)
    .execute(&mut **transaction)
    .await
    .context("failed to assign bootstrap admin role")?;

    if existing.is_none() {
        sqlx::query(
            r#"
            insert into cp_audit_logs (
                audit_id,
                request_id,
                cluster_id,
                actor_id,
                action,
                resource_type,
                resource_id,
                result,
                details,
                created_at
            )
            values (
                $1,
                $2,
                null,
                'system:seed',
                'auth.bootstrap_seeded',
                'user',
                $3,
                'succeeded',
                jsonb_build_object('username', $4, 'role', $5),
                now() - interval '7 days'
            )
            on conflict (audit_id) do nothing
            "#,
        )
        .bind(BOOTSTRAP_ADMIN_SEED_AUDIT_ID)
        .bind(BOOTSTRAP_ADMIN_SEED_REQUEST_ID)
        .bind(BOOTSTRAP_ADMIN_USER_ID)
        .bind(&bootstrap_admin.username)
        .bind(BOOTSTRAP_ADMIN_ROLE_ID)
        .execute(&mut **transaction)
        .await
        .context("failed to record bootstrap admin seed audit log")?;
    }

    Ok(())
}

// Keep bootstrap hashing aligned with the auth service so env-seeded admins can log in normally.
fn hash_password(password: &str) -> Result<String> {
    let mut salt = [0_u8; PASSWORD_SALT_LEN];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| anyhow::anyhow!("failed to generate password salt"))?;

    let mut output = [0_u8; PASSWORD_HASH_LEN];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PASSWORD_ITERATIONS).expect("non-zero iterations"),
        &salt,
        password.as_bytes(),
        &mut output,
    );

    Ok(format!(
        "{PASSWORD_SCHEME}${PASSWORD_ITERATIONS}${}${}",
        URL_SAFE_NO_PAD.encode(salt),
        URL_SAFE_NO_PAD.encode(output)
    ))
}

fn verify_password_hash(password_hash: &str, password: &str) -> Result<bool> {
    let mut parts = password_hash.split('$');
    let scheme = parts.next().context("stored password hash is malformed")?;
    let iterations = parts.next().context("stored password hash is malformed")?;
    let salt = parts.next().context("stored password hash is malformed")?;
    let expected = parts.next().context("stored password hash is malformed")?;

    if scheme != PASSWORD_SCHEME {
        anyhow::bail!("unsupported password scheme `{scheme}`");
    }

    let iterations =
        iterations.parse::<u32>().context("stored password hash has invalid iteration count")?;
    let iterations = NonZeroU32::new(iterations).context("iteration count should be non-zero")?;
    let salt =
        URL_SAFE_NO_PAD.decode(salt).context("stored password hash has invalid salt encoding")?;
    let expected = URL_SAFE_NO_PAD
        .decode(expected)
        .context("stored password hash has invalid hash encoding")?;

    Ok(pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &expected,
    )
    .is_ok())
}

#[cfg(test)]
mod tests {
    use super::{hash_password, verify_password_hash};

    #[test]
    fn password_hash_roundtrip_matches_bootstrap_verifier() {
        let password_hash = hash_password("admin").expect("password hash should be generated");
        assert!(
            verify_password_hash(&password_hash, "admin")
                .expect("password hash should be verified"),
        );
        assert!(
            !verify_password_hash(&password_hash, "change-me-now")
                .expect("password hash should reject wrong passwords"),
        );
    }
}
