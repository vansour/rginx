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
