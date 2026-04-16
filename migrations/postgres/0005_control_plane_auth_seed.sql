insert into cp_roles (role_id, display_name, description)
values
    ('super_admin', 'Super Admin', 'full control-plane access'),
    ('operator', 'Operator', 'operational access for cluster management'),
    ('viewer', 'Viewer', 'read-only access to dashboard and metadata')
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
    ),
    (
        'usr_local_operator',
        'operator',
        'Local Operator',
        'pbkdf2_sha256$100000$6CXGeo7ndc3Jun52pI0yxg$rfLnguC-N1X0yOPTMFDGxGIrtQw9xSvIj_ZSu9GPPsE',
        true,
        now() - interval '7 days',
        now() - interval '7 days'
    ),
    (
        'usr_local_viewer',
        'viewer',
        'Local Viewer',
        'pbkdf2_sha256$100000$kLsTnmdHzzDLUMZm8yA6qQ$zBLIorc9V-84udUw7jNVwy0KHhthgaC5Xt7kLukm_0g',
        true,
        now() - interval '7 days',
        now() - interval '7 days'
    )
on conflict (user_id) do nothing;

insert into cp_user_roles (user_id, role_id)
values
    ('usr_local_admin', 'super_admin'),
    ('usr_local_operator', 'operator'),
    ('usr_local_viewer', 'viewer')
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
    ),
    (
        'audit_auth_seed_0002',
        'seed-auth-0002',
        null,
        'system:seed',
        'auth.bootstrap_seeded',
        'user',
        'usr_local_operator',
        'succeeded',
        jsonb_build_object('username', 'operator', 'role', 'operator'),
        now() - interval '7 days'
    ),
    (
        'audit_auth_seed_0003',
        'seed-auth-0003',
        null,
        'system:seed',
        'auth.bootstrap_seeded',
        'user',
        'usr_local_viewer',
        'succeeded',
        jsonb_build_object('username', 'viewer', 'role', 'viewer'),
        now() - interval '7 days'
    )
on conflict (audit_id) do nothing;
