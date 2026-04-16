# rginx 控制面板实现计划

最后更新：`2026-04-16`

本文档定义 `rginx` 控制面板系统的实现路线。目标是在当前仓库内，以单仓多应用的方式，完整落地：

- 控制台前端：`Vue 3 + Vite`
- 控制面后端：`Axum`
- 持久化数据库：`Postgres`
- 高速状态与任务层：`Dragonfly`
- 控制面唯一部署入口：`Docker`

本文档假设：

- 控制面和边缘节点共享同一仓库。
- 现有 `rginx` 数据面 crate 继续保持独立，不将控制面逻辑塞入 `rginx` 主进程。
- 配置语义、校验和编译能力直接复用 `rginx-config` 与 `rginx-core`，不重复发明一套控制面专用配置模型。

## Phase 0 状态

状态：`Completed`

Phase 0 的目标是冻结控制面边界与架构基线。当前已落地的产物如下：

- 总计划：[ARCHITECTURE_CONTROL_PLANE_IMPLEMENTATION_PLAN.md](ARCHITECTURE_CONTROL_PLANE_IMPLEMENTATION_PLAN.md)
- ADR 索引：[adr/README.md](adr/README.md)
- ADR-0001 单仓多应用边界：[adr/ADR-0001-control-plane-monorepo-boundary.md](adr/ADR-0001-control-plane-monorepo-boundary.md)
- ADR-0002 Postgres / Dragonfly 状态边界：[adr/ADR-0002-control-plane-state-boundary.md](adr/ADR-0002-control-plane-state-boundary.md)
- ADR-0003 Node Agent 拉取模型：[adr/ADR-0003-node-agent-pull-model.md](adr/ADR-0003-node-agent-pull-model.md)
- 环境变量命名规范：[CONTROL_PLANE_ENVIRONMENT_VARIABLES.md](CONTROL_PLANE_ENVIRONMENT_VARIABLES.md)
- API 风格与错误模型：[CONTROL_PLANE_API_CONVENTIONS.md](CONTROL_PLANE_API_CONVENTIONS.md)

Phase 0 完成标准已满足：

- 控制面边界冻结
- 控制面 Docker、边缘节点 APT、Postgres、Dragonfly、Agent 主动连接模型已明确
- 环境变量前缀规划已明确
- API 路径、错误 envelope、错误码风格已明确

## Phase 1 状态

状态：`Completed`

Phase 1 的目标是把控制面收敛到单一 Docker 入口，同时把边缘节点部署路径明确回 APT + systemd。当前已落地的产物如下：

- 主 compose 入口：[compose.yaml](compose.yaml)
- 单一 Docker 构建入口：[Dockerfile](Dockerfile)
- Docker 说明：[docker/README.md](docker/README.md)
- Postgres 初始化 SQL：[migrations/postgres](migrations/postgres)
- Docker 环境变量样例：[.env.example](.env.example)
- Worker 入口 crate：[crates/rginx-control-worker](crates/rginx-control-worker)

Phase 1 完成标准已满足：

- `docker compose` 主入口已存在
- 控制面构建已收敛为单一 `Dockerfile`
- Postgres / Dragonfly / API / worker 已接入统一编排
- 控制面相关镜像统一使用 Debian `trixie` 相关基底
- Dragonfly 统一使用 `ghcr.io` 最新镜像
- 边缘节点默认不通过 Docker 部署，而是保留 APT + systemd 路径
- 从空 volume 启动时，数据库初始化 SQL 会通过 Postgres 官方 initdb 自动执行

## Phase 2 状态

状态：`Completed`

Phase 2 的目标是把控制面后端从单 crate 骨架重构为清晰的 `API -> Service -> Store` 分层，同时把后台 worker 从“预留模块”收敛为独立入口。当前已落地的产物如下：

- HTTP 入口：[crates/rginx-control-api](crates/rginx-control-api)
- 服务层：[crates/rginx-control-service](crates/rginx-control-service)
- 存储层：[crates/rginx-control-store](crates/rginx-control-store)
- 后台任务入口：[crates/rginx-control-worker](crates/rginx-control-worker)

Phase 2 完成标准已满足：

- 控制面已形成 `API -> Service -> Store` 的代码边界
- HTTP handler 不再直接承载业务聚合逻辑
- API 和 worker 已共同复用同一套 service / store 组合
- 当前 Dashboard / health / meta / worker tick 均已通过统一服务层输出

## Phase 3 状态

状态：`Completed`

Phase 3 的目标是把控制面从“有分层骨架”推进到“有真实数据底座”的状态。当前已落地的产物如下：

- Postgres 迁移目录：[migrations/postgres](migrations/postgres)
- 迁移说明：[migrations/README.md](migrations/README.md)
- Postgres-backed store：[crates/rginx-control-store](crates/rginx-control-store)
- Dragonfly keyspace 约定：[CONTROL_PLANE_DRAGONFLY_KEYSPACE.md](CONTROL_PLANE_DRAGONFLY_KEYSPACE.md)

Phase 3 完成标准已满足：

- 节点、配置版本、发布记录、心跳和审计记录已有稳定表结构
- 从空库启动时，迁移和 seed 数据可通过 Docker 入口自动初始化
- `rginx-control-store` 已改为真实 Postgres repository，而不是内存 fixtures
- Dashboard / health / worker runtime 均已通过数据库查询输出

## Phase 4 状态

状态：`Completed`

Phase 4 的目标是给控制面建立最基本的安全边界。当前已落地的产物如下：

- 本地账号与 RBAC 迁移：[migrations/postgres/0004_control_plane_auth_schema.sql](migrations/postgres/0004_control_plane_auth_schema.sql)
- 本地账号 seed：[migrations/postgres/0005_control_plane_auth_seed.sql](migrations/postgres/0005_control_plane_auth_seed.sql)
- 认证服务：[crates/rginx-control-service/src/auth.rs](crates/rginx-control-service/src/auth.rs)
- 路由级鉴权：[crates/rginx-control-api/src/auth.rs](crates/rginx-control-api/src/auth.rs)
- 审计与用户 API：[crates/rginx-control-api/src/routes](crates/rginx-control-api/src/routes)

Phase 4 完成标准已满足：

- 本地登录、登出、当前用户、用户列表、用户创建已打通
- `viewer / operator / super_admin` 三档权限已落到路由守卫
- 登录、登出、失败登录、用户创建已写入审计
- Console 已接入 bearer session 与按角色展示

## Phase 5 状态

状态：`Completed`

Phase 5 的目标是把“控制面能看到节点”这条链路打通。当前已落地的产物如下：

- Phase 5 节点迁移：[migrations/postgres/0006_control_plane_phase5_nodes.sql](migrations/postgres/0006_control_plane_phase5_nodes.sql)
- 节点服务：[crates/rginx-control-service/src/nodes.rs](crates/rginx-control-service/src/nodes.rs)
- agent API：[crates/rginx-control-api/src/routes/agent.rs](crates/rginx-control-api/src/routes/agent.rs)
- 节点列表 API：[crates/rginx-control-api/src/routes/nodes.rs](crates/rginx-control-api/src/routes/nodes.rs)
- 节点本地 agent：[crates/rginx-node-agent](crates/rginx-node-agent)

Phase 5 完成标准已满足：

- API 已提供 `/api/v1/agent/register`、`/api/v1/agent/heartbeat`、`/api/v1/nodes`
- agent 通过独立 shared token 鉴权，不复用控制台 session
- `rginx-node-agent` 已能读取本地 `admin.sock`，并把失败场景上报为 `drifted`
- worker 已按超时阈值把长时间未上报的节点回收为 `offline`
- Console 已能展示节点上线、离线、漂移状态与原因

## Phase 10 状态

状态：`Completed`

Phase 10 的目标是把控制面收口到真正可部署的 Docker 单入口。当前已落地的产物如下：

- 单一编排入口：[compose.yaml](compose.yaml)
- 单一构建入口：[Dockerfile](Dockerfile)
- 统一环境模板：[.env.example](.env.example)
- 单一控制面镜像入口：[docker/control-plane/rginx-control-entrypoint.sh](docker/control-plane/rginx-control-entrypoint.sh)
- Docker 运维说明：[docker/README.md](docker/README.md)

Phase 10 完成标准已满足：

- 控制面开发环境与单机部署统一走 Docker Compose
- `postgres / dragonfly / control-api / control-worker` 已收口到一个 `compose.yaml`
- 运行数据已通过命名卷承载，服务间通信已收口到专用 bridge network
- `postgres / dragonfly / control-api` 已具备健康检查，启动顺序基于健康状态和迁移成功状态
- 根 `Dockerfile` 已收口为单一自研镜像 `rginx-control`；数据库与缓存继续由 Compose 直接编排官方镜像
- Rust 二进制镜像已通过多阶段构建和 `strip` 瘦身，Console 已并入 `control-api` 所在镜像
- 镜像 tag / build metadata 已通过统一环境变量与 OCI labels 收口
- 边缘节点默认继续走 APT + systemd，不加入控制面 Compose

## 1. 总体目标

控制面板系统的最终目标不是“做一个看板”，而是完成一套完整的边缘节点控制面，包括：

- 用户认证与权限控制
- 边缘节点注册、心跳、状态采集
- 运行态快照聚合与控制台展示
- 配置版本管理
- 配置发布、回滚与审计
- Docker-first 的统一开发、测试、部署入口

控制面最终应该形成如下主链路：

`Vue Console -> Axum API -> Service / Worker -> Postgres / Dragonfly -> Node Agent -> local admin.sock -> rginx`

## 2. 核心设计原则

### 2.1 单仓多应用

控制面与边缘节点内核放在同一仓库，但必须拆成多个 crate 和多个容器，不做单进程大杂烩。

### 2.2 Docker 是唯一部署入口

控制面开发环境、测试环境和交付环境统一基于 Docker。  
边缘节点默认仍使用 APT 安装与 systemd 管理，不纳入控制面 `compose.yaml`。

### 2.3 Postgres 是真相源

必须写入 Postgres 的内容：

- 集群
- 节点
- 用户
- 角色
- 配置版本
- 发布任务
- 审计日志

这些数据必须可追溯、可恢复、可审计。

### 2.4 Dragonfly 是加速层，不是真相源

Dragonfly 负责：

- 登录会话
- 任务队列
- 部署锁
- 节点在线状态短缓存
- 实时事件广播
- SSE / WebSocket fanout

不能把只存在于 Dragonfly 的数据当作最终记录。

### 2.5 Agent 主动连接控制面

控制面不直连边缘节点做推送，也不依赖 SSH。  
每个边缘节点部署本地 `node-agent`，主动向控制面注册、发心跳、拉取任务、回报结果。

### 2.6 配置编译只复用已有能力

控制面所有配置编辑、预检、发布前校验，都必须复用：

- `crates/rginx-core`
- `crates/rginx-config`

避免控制面和数据面出现配置语义漂移。

## 3. 目标目录结构

建议在当前仓库收敛为如下结构：

```text
crates/
  rginx-app/
  rginx-config/
  rginx-control-api/
  rginx-control-service/
  rginx-control-store/
  rginx-control-types/
  rginx-control-worker/
  rginx-core/
  rginx-http/
  rginx-node-agent/
  rginx-observability/
  rginx-runtime/
web/
  console/
migrations/
  postgres/
docker/
deploy/
  control-plane/
```

当前仓库已经有：

- `rginx-control-api`
- `rginx-control-service`
- `rginx-control-store`
- `rginx-control-worker`
- `rginx-control-types`
- `rginx-node-agent`
- `web/console`
- `migrations`
- `deploy/control-plane`
- 根目录 `Dockerfile` 与 `compose.yaml`

## 4. 服务职责划分

### 4.1 rginx-control-api

职责：

- HTTP API
- 认证中间件
- 权限校验入口
- 请求/响应 DTO 转换
- OpenAPI 暴露

不负责：

- 复杂业务编排
- 异步发布执行
- 直接拼 SQL

### 4.2 rginx-control-service

职责：

- 领域逻辑
- 节点、配置、部署、审计流程编排
- 事务边界协调

### 4.3 rginx-control-store

职责：

- Postgres repository
- Dragonfly client wrapper
- 锁、队列、会话、事件流等基础访问层

### 4.4 rginx-control-worker

职责：

- 发布任务执行
- 部署批次推进
- 超时处理
- 失败阈值判断
- 自动回滚
- 异步事件广播

### 4.5 rginx-node-agent

职责：

- 节点注册
- 心跳
- 读取本地 `admin.sock`
- 拉取任务
- 写入配置
- 触发 reload
- 上报结果

### 4.6 web/console

职责：

- 用户登录
- Dashboard
- 节点管理
- 版本管理
- 发布管理
- 审计查询

## 5. 数据职责边界

### 5.1 Postgres

建议先落以下表：

- `users`
- `roles`
- `user_roles`
- `api_tokens`
- `clusters`
- `nodes`
- `node_heartbeats`
- `config_revisions`
- `deployments`
- `deployment_batches`
- `deployment_targets`
- `audit_logs`

约束：

- 所有发布状态必须可从数据库重建。
- 所有关键写操作必须可审计。
- 所有配置版本必须可回滚。

### 5.2 Dragonfly

建议先承担以下 keyspace：

- `session:*`
- `deployment:queue:*`
- `deployment:lock:*`
- `node:presence:*`
- `event:stream:*`
- `sse:fanout:*`

约束：

- Dragonfly 重启后，系统不能丢失真实业务记录。
- Dragonfly 中的数据可以重建、过期或回放。

## 6. Docker 部署拓扑

控制面以 Docker 作为唯一部署入口，因此从一开始就按容器角色设计：

### 6.1 控制面容器

- `control-api`
- `control-worker`
- `console`
- `postgres`
- `dragonfly`

### 6.2 边缘节点默认交付形态

- `rginx` 默认通过 APT 安装
- `node-agent` 默认通过主机进程方式运行
- 边缘节点运行时不作为控制面 compose 服务的一部分

### 6.3 关键要求

- `node-agent` 与 `rginx` 共享 `/run/rginx/admin.sock`
- 控制面通过 Docker 网络访问 Postgres 与 Dragonfly
- 所有服务必须定义健康检查
- 所有服务必须通过环境变量配置
- 控制面镜像通过根目录多阶段 `Dockerfile` 构建

## 7. 阶段计划

## Phase 0: 边界冻结与架构基线

状态：`Completed`

目标：

- 冻结控制面边界
- 明确 Docker-first、Postgres、Dragonfly、Agent 主动连接模型
- 冻结 crate 分层和目录规划

输出：

- 本文档
- ADR 文档
- 环境变量命名规范
- API 风格和错误模型约定

完成标准：

- 全团队对控制面边界无歧义
- 后续开发不再反复争论基础拓扑

## Phase 1: Docker-first 基础设施

状态：`Completed`

目标：

- 打通整个控制面的唯一启动入口

输出：

- `compose.yaml`
- 根目录 `Dockerfile`
- `postgres` 初始化方案
- `dragonfly` 配置方案
- 迁移执行入口

完成标准：

- `docker compose up --build` 能拉起整套最小环境
- 新开发者无需手工安装 Postgres / Dragonfly

## Phase 2: 后端分层重构

状态：`Completed`

目标：

- 把控制面后端从单 crate 骨架重构为清晰的三层结构

输出：

- `rginx-control-store`
- `rginx-control-service`
- `rginx-control-worker`
- `rginx-control-api` 精简为 HTTP 层

完成标准：

- 控制面形成 `API -> Service -> Store` 分层
- 业务逻辑不直接散落在 handler 中

当前结果：

- `rginx-control-api` 已收敛为 HTTP 路由和状态注入层
- `rginx-control-service` 已承接 meta / health / dashboard / worker 相关领域逻辑
- `rginx-control-store` 已建立 repository 边界与控制面基础配置入口
- `rginx-control-worker` 已复用同一套 service / store 组合执行后台 tick
- 当前数据访问仍以启动样例数据为主，真实 Postgres / Dragonfly 落地继续放在 Phase 3

## Phase 3: 数据模型与迁移体系

状态：`Completed`

目标：

- 建立稳定的数据库 schema 和迁移流程

输出：

- 全量 Postgres migration
- repository 层
- 初始 seed 数据
- Dragonfly key 规划

完成标准：

- 节点、配置版本、发布记录和审计记录全部可落库
- 可从空库一键初始化

当前结果：

- `migrations/postgres/` 已形成 bootstrap schema、Phase 3 schema 增量和 seed 数据三段迁移
- `cp_clusters`、`cp_nodes`、`cp_node_heartbeats`、`cp_config_revisions`、`cp_deployments`、`cp_audit_logs` 已纳入数据库基线
- `rginx-control-store` 已通过懒连接 Postgres pool 承担真实 repository 查询
- `rginx-control-service`、`rginx-control-api`、`rginx-control-worker` 已切到 async 数据访问路径
- Dragonfly keyspace 已通过代码和文档双重冻结，供后续 queue / lock / presence / SSE 复用

## Phase 4: 认证与权限

状态：`Completed`

目标：

- 给控制面建立最基本的安全边界

输出：

- 本地账号体系
- 会话管理
- RBAC
- 路由级鉴权
- 审计上下文注入

建议初始角色：

- `super_admin`
- `operator`
- `viewer`

完成标准：

- 所有写操作都有权限校验
- 所有写操作都有审计记录

当前结果：

- `cp_users`、`cp_roles`、`cp_user_roles`、`cp_api_sessions` 已进入 Postgres schema
- API 已提供本地登录、登出、当前用户、用户列表和用户创建入口
- `viewer / operator / super_admin` 三档 RBAC 已进入路由级守卫
- 登录、登出、用户创建、失败登录等写操作均会写入 `cp_audit_logs`
- Console 已接入 bearer session，登录后按角色展示 dashboard、meta 和 audit 数据

## Phase 5: Node Agent 与节点注册

状态：`Completed`

目标：

- 把“控制面能看到节点”这条链路打通

输出：

- 节点注册接口
- 心跳接口
- 节点状态机
- 本地 `admin.sock` 适配层
- agent 主机进程与 `rginx` 本地协作方案

完成标准：

- 控制台能看到节点上线、离线、漂移状态
- agent 能稳定读取本地 `rginx` 运行态

当前结果：

- `cp_nodes` 已补充 `admin_socket_path`、`last_snapshot_version`、`runtime_revision`、`runtime_pid`、`status_reason` 等节点运行态摘要字段
- API 已提供 `/api/v1/agent/register` 与 `/api/v1/agent/heartbeat` 两条 agent 专用入口，并通过独立 shared token 鉴权
- `rginx-node-agent` 已能在边缘主机上主动读取本地 `admin.sock`，把 snapshot version、revision、pid、连接数等摘要上报到控制面
- 本地 `admin.sock` 读取失败时，agent 会把节点状态上报为 `drifted` 并附带原因
- `rginx-control-worker` 已按 `RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS` 回收超时未上报节点为 `offline`
- Console 已新增节点列表视图，直接展示在线、离线、漂移状态、最近心跳和漂移原因

## Phase 6: 快照采集与 Dashboard

目标：

- 完成只读型控制台

输出：

- Dashboard
- 节点详情页
- TLS / OCSP 状态页
- listener / vhost / route / upstream 视图
- SSE / WebSocket 实时状态流

完成标准：

- 无需 SSH，即可完成基础运行态诊断

当前结果：

- agent 已从本地 `admin.sock` 拉取完整 snapshot，并通过 `/api/v1/agent/snapshots` 上送控制面
- Postgres 已新增 `cp_node_snapshots`，持久化 `status / counters / traffic / peer_health / upstreams`
- API 已提供 `GET /api/v1/nodes/{node_id}` 聚合节点摘要、最新 snapshot、最近 snapshots 与最近事件
- API 已提供 `GET /api/v1/events` SSE 流，支持 Dashboard 概览与节点详情两类只读实时订阅
- Console 已新增节点详情页与 TLS / OCSP 页面，直接展示 listener、vhost、route、upstream、证书与 OCSP 诊断
- Dashboard 已接入 SSE，节点总览和节点表支持实时刷新并可直接跳转详情页

## Phase 7: 配置版本管理

目标：

- 把配置编辑从文件变成控制面实体

输出：

- 配置草稿
- 配置版本
- diff
- dry-run
- validate / compile
- 版本说明

完成标准：

- 后端可用和数据面一致的语义进行校验和编译
- 可安全生成可发布 revision

当前结果：

- Postgres 已新增 `cp_config_drafts`，并为 `cp_config_revisions` 补充 `source_path / config_text / compile_summary`
- API 已提供 draft / revision 管理入口，支持草稿创建、更新、校验、diff、发布
- `rginx-control-service` 已直接复用 `rginx-config::load_and_compile_from_str` 做 validate / compile dry-run
- draft 校验结果会持久化为 `valid / invalid / published` 状态和 compile summary
- Console 已新增 Revision 管理页，支持：
  - 草稿创建与编辑
  - validate / compile
  - 对 base revision 或最新 revision 做 diff
  - 将通过校验的 draft 发布成 revision

## Phase 8: 发布编排与回滚

目标：

- 做真正可用的发布控制面

建议流程：

`create deployment -> queue in Dragonfly -> worker dispatch -> agent poll -> apply config -> reload -> report -> aggregate result`

输出：

- 分批发布
- 并发窗口
- 暂停 / 继续
- 失败阈值
- 自动回滚
- 幂等键
- 部署锁

完成标准：

- 支持单集群滚动发布
- 支持失败中止和回滚到上一个 revision

当前结果：

- Postgres 已新增 `cp_deployment_targets` 与 `cp_agent_tasks`，并为 `cp_deployments` 补充：
  - `parallelism`
  - `failure_threshold`
  - `auto_rollback`
  - `rollback_of_deployment_id`
  - `rollback_revision_id`
  - `status_reason`
  - `idempotency_key`
- API 已提供：
  - `GET /api/v1/deployments`
  - `POST /api/v1/deployments`
  - `GET /api/v1/deployments/{deployment_id}`
  - `POST /api/v1/deployments/{deployment_id}/pause`
  - `POST /api/v1/deployments/{deployment_id}/resume`
  - `POST /api/v1/agent/tasks/poll`
  - `POST /api/v1/agent/tasks/{task_id}/ack`
  - `POST /api/v1/agent/tasks/{task_id}/complete`
- `rginx-control-worker` 已能周期推进 deployment：
  - 按并发窗口分批派发 target task
  - 在失败阈值达到后停止继续派发并取消未开始目标
  - 聚合节点成功 / 失败结果并收口 deployment 状态
  - 在 `auto_rollback = true` 时为已成功节点自动创建 rollback deployment
- `rginx-node-agent` 已能主动拉取任务并在边缘主机本地执行：
  - 拉取 task
  - ack
  - 写 staging config
  - `rginx --config <staging> check`
  - 切换 live config
  - `rginx --config <live> -s reload`
  - 失败时恢复备份配置并再次 reload
  - complete 回报成功 / 失败
- Console 已新增 Deployment 页面，支持：
  - 创建 deployment
  - 查看 deployment detail / target 状态 / recent events
  - 暂停 / 继续
  - 从 Revision 页面直接跳转发起发布

## Phase 9: 审计、告警与运维能力

目标：

- 把系统从“能用”推进到“可托管”

输出：

- 完整 audit log
- 发布事件追踪
- 节点异常告警
- Prometheus metrics
- 结构化日志
- 备份恢复手册

完成标准：

- 可以回答“谁在什么时候对哪个集群发布了什么版本，结果如何”

当前结果：

- API 已提供完整审计查询入口：
  - `GET /api/v1/audit-logs`
  - `GET /api/v1/audit-logs/{audit_id}`
  - 支持 `cluster_id / actor_id / action / resource_type / resource_id / result / limit` 过滤
- deployment 详情中的 `recent_events` 已升级为真正的 timeline，覆盖：
  - deployment 主事件
  - agent task ack / complete 事件
- SSE 已支持按 deployment 订阅：
  - `GET /api/v1/events?deployment_id=<id>&access_token=<token>`
  - 事件类型：`deployment.tick`
- 控制面已新增派生告警能力：
  - `GET /api/v1/alerts`
  - 当前覆盖离线节点、drifted 节点、失败 deployment、暂停 deployment、运行中但需要人工关注的 deployment
- 控制面已新增 Prometheus 抓取端点：
  - `GET /metrics`
  - 当前暴露集群、节点、deployment、alert、revision、audit log、snapshot 等核心 gauge
- `rginx-control-api` 已新增统一 request logging middleware：
  - 为每个请求生成或继承 `request_id`
  - 在响应头回写 `x-request-id`
  - 输出 `request_id / method / path / status / elapsed_ms / user_agent / remote_addr` 结构化日志
- Console 已新增：
  - Dashboard 告警视图
  - Audit 页面
  - Deployment 页面实时事件追踪
- 仓库已新增备份恢复手册：
  - [CONTROL_PLANE_BACKUP_AND_RECOVERY.md](CONTROL_PLANE_BACKUP_AND_RECOVERY.md)

## Phase 10: Docker 生产化收口

目标：

- 兑现控制面 Docker 是唯一部署入口

输出：

- 单一 `compose.yaml`
- 单一根目录 `Dockerfile`
- 统一 `.env` 模板
- 数据卷和网络设计
- 健康检查与启动顺序
- 镜像瘦身与发布策略

完成标准：

- 控制面开发环境与单机部署统一走 Docker Compose
- 边缘节点默认走 APT + systemd

## 8. 推荐的阶段切分策略

建议分三轮推进：

### 第一轮：做到 Phase 6

目标：

- 登录
- 节点可见
- 快照可见
- Dashboard 可用

原因：

- 先把只读链路打通，可以尽快验证 agent、控制面、数据库、实时状态链路是否合理。

### 第二轮：做到 Phase 7 和 Phase 8

目标：

- 配置版本管理
- 发布编排
- 回滚

原因：

- 配置和部署是复杂度最高的部分，必须在只读链路稳定后再做。

### 第三轮：做到 Phase 9 和 Phase 10

目标：

- 生产化运维
- 完整交付与托管能力

原因：

- 这是平台化收口阶段，不应该和最初的 MVP 混在一起推进。

## 9. 第一批已落地的代码包

当前首批控制面代码包已经在仓库内落位：

- `rginx-control-store`
- `rginx-control-service`
- `rginx-control-worker`
- `rginx-control-api`
- `rginx-node-agent`
- `web/console`
- `docker/*`
- `migrations/*`

## 10. 近期落地优先级

如果立即开工，建议优先顺序如下：

1. agent 注册、心跳与节点状态机
2. Dashboard 扩展到节点详情和审计查询
3. 配置版本创建、校验与 revision 审批
4. deployment queue / lock / event stream 真正接入 Dragonfly
5. 发布编排、批次推进与回滚

## 11. 明确不做的事情

在控制面一期里，不建议一开始就做：

- 多活控制面集群
- 多租户资源计费
- 全量 OIDC / SSO 集成
- 复杂策略引擎
- 跨区域智能调度
- 节点自动扩缩容平台

这些能力都可以作为后续阶段扩展，但不应该阻塞控制面的第一版落地。

## 12. 结论

`rginx` 控制面应该沿着以下方向推进：

- 仓库内实现，而不是另起仓库
- 单仓多应用，而不是同进程混合
- Postgres 做真相源
- Dragonfly 做加速层
- Docker 做唯一部署入口
- Node Agent 主动连接控制面
- 配置编译严格复用现有数据面能力

这条路径能最大限度复用现有 `rginx` 的配置模型、运行时快照和 reload/restart 语义，同时避免控制面与数据面逐渐分叉。
