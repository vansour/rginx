# Control Plane Schema Bootstrap

当前目录保存控制面数据库初始化 SQL。

- 默认假设控制面使用 PostgreSQL。
- 迁移文件放在 `migrations/postgres/`。
- 当前已分为三类：
  - `0001_*`：基础 bootstrap schema
  - `0002_*`：Phase 3 schema 增量，例如心跳表和审计表
  - `0003_*`：本地开发和 Docker 启动使用的最小 seed 数据
  - `0004_*` / `0005_*`：Phase 4 本地账号、会话与 RBAC
  - `0006_*`：Phase 5 节点注册与运行态摘要字段
  - `0007_*`：Phase 6 节点完整 snapshot 持久化与 Dashboard 只读诊断字段
  - `0008_*`：Phase 7 配置 draft / revision 管理、validate / compile 元数据
  - `0009_*`：Phase 8 发布目标、agent task、deployment rollback / idempotency / pause-resume 字段

当前数据库基线包含：

- `cp_clusters`
- `cp_nodes`
- `cp_node_heartbeats`
- `cp_config_revisions`
- `cp_deployments`
- `cp_audit_logs`
- `cp_users`
- `cp_roles`
- `cp_user_roles`
- `cp_api_sessions`
- `cp_node_snapshots`
- `cp_config_drafts`
- `cp_deployment_targets`
- `cp_agent_tasks`

根目录 `compose.yaml` 中的 `postgres` 服务会在空 volume 首次启动时，通过官方
`/docker-entrypoint-initdb.d` 机制按文件名字典序依次执行这些 SQL 文件。
