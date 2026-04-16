# Docker Entry Points

Phase 10 起，控制面开发环境和单机部署统一走仓库根目录的 `compose.yaml`。
边缘节点默认沿用 APT 安装与 systemd 管理，不纳入 `compose.yaml`。

## 当前约定

- 主入口是仓库根目录的 `compose.yaml`
- 所有控制面自研产物统一从仓库根目录的 `Dockerfile` 构建为单一镜像 `rginx-control`
- Rust 运行镜像统一收口到 Debian `trixie-slim`
- 前端构建统一使用 `node:24-trixie`，构建产物直接并入 `rginx-control` 镜像并由 API 服务
- Postgres 统一使用 `postgres:18-trixie`
- Dragonfly 统一使用 `ghcr.io/dragonflydb/dragonfly:latest`
- `docker/` 目录只保留辅助脚本、Nginx 模板和文档，不再保存第二套 Compose / Dockerfile

## 编排内容

- `postgres`：真相源，命名卷 `postgres-data`
- `dragonfly`：状态 / 队列加速层，命名卷 `dragonfly-data`
- `control-api`：基于 `rginx-control` 镜像运行，负责 Axum API 和内嵌 Console 静态资源
- `control-worker`：基于同一个 `rginx-control` 镜像运行，负责异步任务与状态回收

所有服务运行在同一个命名 bridge network `control-plane` 上，Compose 默认启用健康检查和依赖顺序：

- `postgres` 必须完成官方 initdb 初始化并通过 schema healthcheck
- `dragonfly` 必须先 healthy

首次空卷启动时，`postgres` 会自动按文件名字典序执行 `migrations/postgres/*.sql`。
仓库不再维护独立迁移脚本或 `migrate` 服务，Console 也不再单独占用一个容器。

## 最小启动

```bash
cp .env.example .env
docker compose --env-file .env up -d --build
```

默认端口只绑定到本机回环地址：

- Console + Control API: `127.0.0.1:8080`
- Postgres: `127.0.0.1:5432`
- Dragonfly: `127.0.0.1:6379`

如需对外暴露，调整 `.env` 中的 `RGINX_CONTROL_API_PUBLISH` 等变量即可。

## 运维动作

```bash
docker compose ps
docker compose logs -f control-api control-worker
docker compose down
```

## 镜像发布

`.env.example` 中的以下变量用于统一镜像标识：

- `RGINX_CONTROL_IMAGE_TAG`
- `RGINX_BUILD_VERSION`
- `RGINX_BUILD_REVISION`
- `RGINX_BUILD_CREATED`

推荐流程：

```bash
docker compose build
docker compose push control-api
```

Rust 二进制在构建阶段会执行 `strip`，前端采用静态构建后直接并入同一个 `rginx-control` 镜像，避免再维护独立 Console 镜像。数据库和缓存继续直接使用官方 `postgres:18-trixie` 与 `ghcr.io/dragonflydb/dragonfly:latest` 镜像，不再混入根 `Dockerfile`。

Phase 4 起，Console 默认需要先登录后才能读取 `/api/v1/dashboard`、`/api/v1/meta` 等受保护接口。

Phase 5 起，`control-api` 还要求配置 `RGINX_CONTROL_AGENT_SHARED_TOKEN`，供边缘节点上的
`rginx-node-agent` 通过 `/api/v1/agent/register` 和 `/api/v1/agent/heartbeat` 接入控制面。
边缘节点默认仍通过 APT + systemd 部署 agent 和 `rginx`，不加入 `compose.yaml`。
