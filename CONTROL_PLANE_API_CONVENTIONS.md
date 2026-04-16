# 控制面 API 风格与错误模型约定

最后更新：`2026-04-16`

本文档定义控制面 Phase 0 的 API 基线，作为后续 `axum` API、前端 Console、Node Agent 和 Worker 之间的统一约定。

## 1. API 总原则

- HTTP API 统一走 JSON
- 版本化前缀统一使用 `/api/v1`
- 健康检查不放在版本化路径下
- 对外 API 和 agent API 可以共用同一服务，但路由空间必须分离
- 所有写接口默认要求认证

## 2. 路由空间

### 2.1 基础路由

- `GET /healthz`
- `GET /readyz`
- `GET /livez`

### 2.2 控制台 API

- `/api/v1/auth/*`
- `/api/v1/users`
- `/api/v1/dashboard`
- `/api/v1/alerts`
- `/api/v1/clusters`
- `/api/v1/nodes`
- `/api/v1/revisions`
- `/api/v1/deployments`
- `/api/v1/audit-logs`

### 2.3 Agent API

- `/api/v1/agent/register`
- `/api/v1/agent/heartbeat`
- `/api/v1/agent/tasks/poll`
- `/api/v1/agent/tasks/{task_id}/ack`
- `/api/v1/agent/tasks/{task_id}/complete`
- `/api/v1/agent/snapshots`

## 3. 命名规则

- 集合资源使用复数：`/nodes`
- 单资源使用路径参数：`/nodes/{node_id}`
- 动作语义只在确实不是 CRUD 时使用，例如 `/poll`、`/complete`
- 路由参数统一使用小写蛇形或业务 id，不使用驼峰

## 4. 成功响应约定

### 4.1 单对象

```json
{
  "data": {
    "node_id": "edge-sha-01"
  }
}
```

### 4.2 列表

```json
{
  "data": [
    {
      "node_id": "edge-sha-01"
    }
  ],
  "meta": {
    "next_cursor": null
  }
}
```

### 4.3 长任务创建

```json
{
  "data": {
    "deployment_id": "deploy_0001",
    "status": "running"
  }
}
```

## 5. 错误模型

所有错误统一返回如下 envelope：

```json
{
  "error": {
    "code": "node.not_found",
    "message": "node `edge-sha-01` was not found",
    "request_id": "req_01jxyz",
    "retryable": false,
    "details": {
      "node_id": "edge-sha-01"
    }
  }
}
```

字段定义：

- `code`: 稳定的机器可读错误码
- `message`: 面向调用方的简洁说明
- `request_id`: 用于追踪日志
- `retryable`: 是否建议自动重试
- `details`: 可选的结构化上下文

## 6. 错误码风格

错误码统一使用：

- `<domain>.<reason>`

示例：

- `auth.unauthorized`
- `auth.forbidden`
- `node.not_found`
- `node.already_registered`
- `revision.invalid_config`
- `deployment.concurrent_operation`
- `deployment.lock_not_acquired`
- `internal.unexpected`

## 7. HTTP 状态码映射

- `400 Bad Request`: 请求格式错误、参数错误、状态不合法
- `401 Unauthorized`: 未认证
- `403 Forbidden`: 已认证但无权限
- `404 Not Found`: 资源不存在
- `409 Conflict`: 并发冲突、幂等冲突、状态冲突
- `422 Unprocessable Entity`: 配置语义错误、校验失败
- `429 Too Many Requests`: 速率限制
- `500 Internal Server Error`: 未预期错误
- `503 Service Unavailable`: 下游依赖不可用、节点未就绪

## 8. 幂等约定

以下写接口必须支持幂等：

- 创建 revision
- 创建 deployment
- agent 提交任务完成结果

约定：

- 使用 `Idempotency-Key` 请求头
- 幂等记录落 Postgres 或 Dragonfly，但最终结果必须可回查

## 9. 请求追踪

每个请求都应具备：

- `request_id`
- `actor_id`
- `cluster_id` 或资源归属

如果客户端未提供 `X-Request-ID`，后端必须自动生成。

## 10. 分页与筛选

- 列表接口默认使用 cursor 分页
- 简单管理接口可在早期阶段先接受 `limit`
- 过滤参数统一使用 query string

示例：

`GET /api/v1/nodes?cluster_id=cluster-mainland&state=online&limit=50`

## 11. 时间格式

- API 返回时间统一使用 `unix_ms`
- 如需面向用户展示，由前端负责格式化

## 12. 实时事件

控制台实时状态更新优先使用 SSE，保留 WebSocket 作为后续增强选项。

推荐路由：

- `POST /api/v1/events/session`
- `GET /api/v1/events`

推荐事件类型：

- `node.presence_changed`
- `deployment.updated`
- `deployment.finished`
- `deployment.tick`
- `snapshot.updated`

## 13. Agent 鉴权

Phase 0 基线要求：

- agent 和控制面必须使用独立的鉴权机制，不和用户登录会话混用
- agent 后续优先采用 mTLS 或签名 token
- agent 接口和 console 接口必须在中间件层面分离

## 14. 不做的事情

在 Phase 0 不强制定义：

- 完整 OpenAPI 生成流程
- 多租户路径模型
- GraphQL
- 跨区域事件总线

但后续实现不得违反本文档中的路径、错误码和 envelope 基线。

当前 Phase 4 已落地的认证相关路由：

- `POST /api/v1/auth/login`
- `POST /api/v1/auth/logout`
- `GET /api/v1/auth/me`
- `GET /api/v1/users`
- `POST /api/v1/users`

当前 Phase 5 已落地的节点相关路由：

- `GET /api/v1/nodes`
- `POST /api/v1/agent/register`
- `POST /api/v1/agent/heartbeat`

当前 Phase 6 已落地的节点观测路由：

- `GET /api/v1/nodes/{node_id}`
- `POST /api/v1/agent/snapshots`
- `GET /api/v1/events`

当前 Phase 7 已落地的配置版本管理路由：

- `GET /api/v1/revisions`
- `GET /api/v1/revisions/{revision_id}`
- `GET /api/v1/revisions/drafts`
- `POST /api/v1/revisions/drafts`
- `GET /api/v1/revisions/drafts/{draft_id}`
- `PUT /api/v1/revisions/drafts/{draft_id}`
- `POST /api/v1/revisions/drafts/{draft_id}/validate`
- `GET /api/v1/revisions/drafts/{draft_id}/diff`
- `POST /api/v1/revisions/drafts/{draft_id}/publish`

当前 Phase 8 已落地的发布编排路由：

- `GET /api/v1/deployments`
- `POST /api/v1/deployments`
- `GET /api/v1/deployments/{deployment_id}`
- `POST /api/v1/deployments/{deployment_id}/pause`
- `POST /api/v1/deployments/{deployment_id}/resume`
- `POST /api/v1/agent/tasks/poll`
- `POST /api/v1/agent/tasks/{task_id}/ack`
- `POST /api/v1/agent/tasks/{task_id}/complete`

当前 Phase 9 已落地的审计 / 告警 / 运维路由：

- `GET /api/v1/alerts`
- `GET /api/v1/audit-logs`
- `GET /api/v1/audit-logs/{audit_id}`
- `GET /metrics`

当前 `/api/v1/events` 已支持的 query 维度：

- `node_id`
- `deployment_id`
