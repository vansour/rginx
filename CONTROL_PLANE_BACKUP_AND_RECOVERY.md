# 控制面备份与恢复手册

最后更新：`2026-04-16`

本文档覆盖控制面 Phase 9 的最小可托管备份恢复流程。

## 1. 备份范围

必须备份：

- Postgres 数据库
- `.env` 中的控制面密钥与口令
- 根目录 [compose.yaml](/root/github/rginx/compose.yaml)
- 根目录 [Dockerfile](/root/github/rginx/Dockerfile)
- 控制面运维文档和本地覆盖配置

可以不备份：

- Dragonfly 临时数据

原因：

- Postgres 是控制面的真相源
- Dragonfly 只承载队列、锁、短期缓存和 fanout，可由 Postgres 状态重新驱动

## 2. 推荐备份方式

### 2.1 PostgreSQL 逻辑备份

```bash
docker compose exec postgres \
  pg_dump -U "$RGINX_CONTROL_DB_USER" -d "$RGINX_CONTROL_DB_NAME" -Fc \
  > backup-control-plane.dump
```

### 2.2 环境与部署文件备份

```bash
tar czf backup-control-plane-configs.tar.gz \
  .env compose.yaml Dockerfile \
  CONTROL_PLANE_API_CONVENTIONS.md \
  CONTROL_PLANE_DRAGONFLY_KEYSPACE.md \
  CONTROL_PLANE_ENVIRONMENT_VARIABLES.md \
  CONTROL_PLANE_BACKUP_AND_RECOVERY.md
```

## 3. 恢复前检查

恢复前先确认：

- 已停止 `control-api` 与 `control-worker`
- 待恢复实例的 `.env` 与镜像版本一致
- 不在目标环境上再次执行 seed 覆盖操作

推荐先停服务：

```bash
docker compose stop control-api control-worker
```

## 4. PostgreSQL 恢复

### 4.1 清空并重建目标库

```bash
docker compose exec postgres psql -U "$RGINX_CONTROL_DB_USER" -d postgres -c \
  "drop database if exists $RGINX_CONTROL_DB_NAME;"

docker compose exec postgres psql -U "$RGINX_CONTROL_DB_USER" -d postgres -c \
  "create database $RGINX_CONTROL_DB_NAME;"
```

### 4.2 导入逻辑备份

```bash
cat backup-control-plane.dump | docker compose exec -T postgres \
  pg_restore -U "$RGINX_CONTROL_DB_USER" -d "$RGINX_CONTROL_DB_NAME" --clean --if-exists
```

## 5. Dragonfly 处理

可选操作：

- 直接保留现有 Dragonfly 并重启 worker
- 或在恢复时清空 Dragonfly，让 worker 从 Postgres 重新建立临时状态

如果明确需要清空：

```bash
docker compose exec dragonfly redis-cli FLUSHALL
```

## 6. 恢复后启动顺序

```bash
docker compose up -d postgres dragonfly
docker compose up -d control-api control-worker
```

说明：

- 当前 compose 拓扑没有独立的 `console` 或 `migrate` 服务，Console 静态资源已并入 `control-api`
- Postgres 迁移脚本只在空数据目录初始化时由 `docker-entrypoint-initdb.d` 自动执行；恢复现有库时不应再假设会有额外迁移容器
- 控制面真实业务状态以已恢复的 Postgres 数据为准

## 7. 恢复后验证

至少检查以下项目：

- `curl -sf http://127.0.0.1:8080/healthz`
- `curl -sf http://127.0.0.1:8080/metrics | head`
- Console 可以登录并打开 Dashboard
- `/api/v1/audit-logs` 能查询到历史审计记录
- `/api/v1/deployments` 能查到恢复前的 deployment 历史
- worker 日志中没有持续的 Postgres / Dragonfly 错误

## 8. 边缘节点说明

- 边缘节点默认不走 Docker
- 边缘节点上的 `rginx` 配置、systemd unit 和 `rginx-node-agent` 运行文件属于节点侧运维资产，不在控制面 Docker 备份范围内
- 控制面恢复后，节点会继续通过 heartbeat、snapshot 和 task poll 重新对齐状态
