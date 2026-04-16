# 控制面环境变量命名规范

最后更新：`2026-04-16`

本文档定义控制面 Phase 0 的环境变量命名基线。目标不是一次性列出所有变量，而是先冻结命名规则、作用域和默认约束，避免后续命名漂移。

## 1. 总规则

- 所有控制面变量统一使用大写蛇形命名
- 统一使用 `RGINX_` 前缀
- 数据面变量和控制面变量必须按职责分组
- 不在环境变量里存放结构化 JSON，复杂结构优先使用文件或数据库
- 所有时间相关变量使用显式单位后缀，例如 `_SECS`、`_MS`
- 布尔变量统一使用 `true` / `false`

## 2. 前缀规划

### 2.1 Control API

前缀：

- `RGINX_CONTROL_API_`

用于：

- HTTP bind 地址
- CORS
- request id
- 公开 base url
- SSE / WebSocket 限制

示例：

- `RGINX_CONTROL_API_ADDR`
- `RGINX_CONTROL_API_PUBLIC_ORIGIN`
- `RGINX_CONTROL_API_REQUEST_TIMEOUT_SECS`

### 2.2 Control Worker

前缀：

- `RGINX_CONTROL_WORKER_`

用于：

- 队列轮询周期
- deployment 超时
- 清理任务周期

示例：

- `RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS`

### 2.3 Postgres

前缀：

- `RGINX_CONTROL_DB_`

用于：

- host / port / user / password / database
- 连接池
- 迁移开关

示例：

- `RGINX_CONTROL_DB_HOST`
- `RGINX_CONTROL_DB_PORT`
- `RGINX_CONTROL_DB_USER`
- `RGINX_CONTROL_DB_PASSWORD`
- `RGINX_CONTROL_DB_NAME`
- `RGINX_CONTROL_DB_MAX_CONNECTIONS`

### 2.4 Dragonfly

前缀：

- `RGINX_CONTROL_DRAGONFLY_`

用于：

- host / port / password
- db index
- key prefix
- queue 名称

示例：

- `RGINX_CONTROL_DRAGONFLY_HOST`
- `RGINX_CONTROL_DRAGONFLY_PORT`
- `RGINX_CONTROL_DRAGONFLY_PASSWORD`
- `RGINX_CONTROL_DRAGONFLY_KEY_PREFIX`

### 2.5 Auth

前缀：

- `RGINX_CONTROL_AUTH_`

用于：

- session secret
- token TTL
- agent shared token
- cookie 配置
- OIDC 预留开关

示例：

- `RGINX_CONTROL_AUTH_SESSION_SECRET`
- `RGINX_CONTROL_AUTH_SESSION_TTL_SECS`
- `RGINX_CONTROL_AGENT_SHARED_TOKEN`
- `RGINX_CONTROL_AUTH_COOKIE_SECURE`

### 2.6 Node Agent

前缀：

- `RGINX_NODE_AGENT_`

用于：

- node id
- cluster id
- advertise addr
- 节点角色
- 节点运行版本
- control plane origin
- admin socket 路径
- `rginx` 二进制路径
- live config 路径
- backup / staging 目录
- 默认生命周期状态
- heartbeat 周期
- HTTP 请求超时
- task poll 周期

示例：

- `RGINX_NODE_ID`
- `RGINX_CLUSTER_ID`
- `RGINX_NODE_ADVERTISE_ADDR`
- `RGINX_NODE_ROLE`
- `RGINX_NODE_RUNNING_VERSION`
- `RGINX_NODE_LIFECYCLE_STATE`
- `RGINX_CONTROL_PLANE_ORIGIN`
- `RGINX_ADMIN_SOCKET`
- `RGINX_NODE_BINARY`
- `RGINX_NODE_CONFIG_PATH`
- `RGINX_NODE_CONFIG_BACKUP_DIR`
- `RGINX_NODE_CONFIG_STAGING_DIR`
- `RGINX_NODE_AGENT_HEARTBEAT_SECS`
- `RGINX_NODE_AGENT_REQUEST_TIMEOUT_SECS`
- `RGINX_NODE_AGENT_TASK_POLL_SECS`

### 2.7 前端 Console

前缀：

- `VITE_RGINX_CONSOLE_`

用于：

- API base url
- SSE endpoint
- 默认租户或环境标识

示例：

- `VITE_RGINX_CONSOLE_API_BASE_URL`
- `VITE_RGINX_CONSOLE_SSE_URL`

## 3. 必填与默认值规则

- 与部署环境强绑定的变量允许无默认值，例如密码、secret、public origin
- 与本地开发便利性相关的变量可以有默认值，例如 `127.0.0.1:8080`
- 所有无默认值的变量必须在服务启动时显式失败

## 4. 禁止事项

- 禁止混用 `RGINX_API_*`、`RGINX_CP_*`、`CONTROL_*` 这类非统一前缀
- 禁止用 `_TIMEOUT` 但不标单位
- 禁止用 `0/1` 表示布尔值
- 禁止在 agent 和 control-api 间复用含义不同但名字相同的变量

## 5. 示例基线

```env
RGINX_CONTROL_API_ADDR=0.0.0.0:8080
RGINX_CONTROL_API_PUBLIC_ORIGIN=https://console.example.com

RGINX_CONTROL_DB_HOST=postgres
RGINX_CONTROL_DB_PORT=5432
RGINX_CONTROL_DB_USER=rginx
RGINX_CONTROL_DB_PASSWORD=change-me
RGINX_CONTROL_DB_NAME=rginx_control
RGINX_CONTROL_DB_MAX_CONNECTIONS=10

RGINX_CONTROL_DRAGONFLY_HOST=dragonfly
RGINX_CONTROL_DRAGONFLY_PORT=6379
RGINX_CONTROL_DRAGONFLY_KEY_PREFIX=rginx:control

RGINX_CONTROL_AUTH_SESSION_SECRET=replace-me
RGINX_CONTROL_AUTH_SESSION_TTL_SECS=86400
RGINX_CONTROL_AGENT_SHARED_TOKEN=replace-me-for-agent
RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS=30

RGINX_NODE_ID=edge-dev-01
RGINX_CLUSTER_ID=cluster-mainland
RGINX_NODE_ADVERTISE_ADDR=10.0.0.11:8443
RGINX_NODE_ROLE=edge
RGINX_NODE_RUNNING_VERSION=v0.1.3-rc.11
RGINX_NODE_LIFECYCLE_STATE=online
RGINX_CONTROL_PLANE_ORIGIN=http://control-api:8080
RGINX_ADMIN_SOCKET=/run/rginx/admin.sock
RGINX_NODE_BINARY=/usr/sbin/rginx
RGINX_NODE_CONFIG_PATH=/etc/rginx/rginx.ron
RGINX_NODE_CONFIG_BACKUP_DIR=/var/lib/rginx-node-agent/backups
RGINX_NODE_CONFIG_STAGING_DIR=/var/lib/rginx-node-agent/staging
RGINX_NODE_AGENT_HEARTBEAT_SECS=10
RGINX_NODE_AGENT_REQUEST_TIMEOUT_SECS=5
RGINX_NODE_AGENT_TASK_POLL_SECS=3
```
