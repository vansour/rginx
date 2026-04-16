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
