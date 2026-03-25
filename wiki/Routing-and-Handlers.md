# Routing and Handlers

本页说明请求如何匹配到虚拟主机、如何匹配到路由，以及七种 handler 的行为边界。

## 路由匹配模型

`rginx` 当前只支持两种 matcher：

- `Exact("/foo")`
- `Prefix("/api")`

在此基础上，路由还可以额外声明：

- `grpc_service`
- `grpc_method`

这两个字段不会替代路径 matcher，而是在路径命中后，对 gRPC / grpc-web 请求再做一层细分匹配。

不支持：

- 正则匹配
- `~` / `~*`
- `^~` 单独语义

## 匹配顺序

编译后的路由会按优先级排序：

1. `Exact`
2. `Prefix`
3. 同类型下，路径更长的优先
4. 同一路径 matcher 下，带更多 gRPC 约束的优先

因此：

- `/api` 的精确匹配会优先于 `/`
- `/api/admin` 的前缀会优先于 `/api`
- `Prefix("/") + grpc_service = "grpc.health.v1.Health" + grpc_method = "Check"` 会优先于只配置了 `grpc_service` 的 `/` 路由

## 前缀匹配的边界

`Prefix("/api")` 会匹配：

- `/api`
- `/api/demo`

不会匹配：

- `/apix`

也就是说，前缀匹配遵守 segment boundary，而不是单纯的字符串 starts_with。

## 虚拟主机选择

流程是：

1. 取 `Host` 头；如果没有，则回退到 URI authority。
2. 在 `servers` 中找匹配的 `server_names`。
3. 未匹配时，回退到默认虚拟主机，也就是顶层 `server + locations`。

支持：

- 精确域名
- `*.example.com` 通配符

重要语义：

- 如果某个虚拟主机已经命中 `Host`，但里面没有匹配路径，请求直接返回 `404`
- 不会因为路由未命中而回退到默认虚拟主机

## 七种 handler

### `Static`

直接返回固定响应：

- 状态码
- `content-type`
- `body`

适合：

- 健康检查
- 简单 landing page
- 临时占位页
- 内网调试响应

### `Proxy`

把请求转发到命名 upstream。

可配项：

- `upstream`
- `preserve_host`
- `strip_prefix`
- `proxy_set_headers`

行为要点：

- 自动写入或追加 `X-Forwarded-For`
- 自动设置 `X-Forwarded-Proto`
- 自动设置 `X-Forwarded-Host`
- 自动透传 `X-Request-ID`
- 支持 HTTP Upgrade / WebSocket 透传
- gRPC / grpc-web 请求可以继续按 `grpc_service` / `grpc_method` 细分到不同 upstream

### `File`

从本地文件系统返回内容。

支持：

- `root`
- `index`
- `try_files`
- `autoindex`
- `HEAD`
- 单段 `Range`
- `206 Partial Content`

安全边界：

- 对请求路径做 percent decode
- 拒绝路径穿越
- 默认只返回真实文件；只有显式配置 `autoindex: Some(true)` 时才会返回基础目录列表 HTML
- 目录命中后的优先级是 `index > try_files > autoindex > 404`

不支持：

- 多段 `Range`

### `Return`

立即返回指定状态码、`Location` 和可选响应体。

适合：

- HTTP 跳转
- 简单的固定错误页
- 兼容旧路径

### `Status`

输出 JSON 状态页，典型用于内网自检和控制面查看。

包含：

- 配置修订号
- 监听地址
- TLS 是否启用
- keepalive 是否启用
- 当前连接上限
- trusted proxy 条目数
- 当前活跃客户端连接数
- vhost 数
- route 数
- upstream 数
- 每个 upstream 的 peer 总数、健康 peer 数、backup peer 数、当前活跃请求汇总，以及 transport / pool / health 配置
- 每个 peer 的 `weight`、`backup`、健康状态和当前活跃请求数

### `Metrics`

输出 Prometheus 文本指标，适合被 Prometheus、VictoriaMetrics、Grafana Agent 等抓取。

当前包含基础 gRPC 计数指标 `rginx_grpc_requests_total`，按 `route`、`protocol`、`service`、`method` 维度累计。

约定：

- 保持 metric name 稳定，优先扩文档和排障语义，而不是频繁改名
- label 只放低基数维度，如 `route`、`status`、`protocol`、`service`、`method`、`upstream`、`peer`、`result`
- 不要把 `request_id`、客户端 IP、原始 path、host 等高基数字段放进指标 label

### `Config`

提供基础动态配置 API，典型用于受控内网里的运维入口。

支持：

- `GET` / `HEAD` 读取当前生效配置和修订号
- `PUT` 提交完整 RON 文档并尝试在线替换

安全边界：

- 路由必须使用 `Exact(...)`
- 必须显式配置非空 `allow_cidrs`
- `PUT` body 必须是非空、有效 UTF-8 的完整 RON 文档
- `PUT` body 当前限制为 1 MiB；超限会返回 `413 Payload Too Large`
- 当前只支持完整文档替换，不支持 partial patch
- 仍然不能在线切换 `listen`、`runtime.worker_threads`、`runtime.accept_workers`

## `Proxy` 相关细节

### `preserve_host`

- 默认情况下，`Host` 会改写为 upstream authority
- 开启后，保留原始 `Host`

### `strip_prefix`

例如：

- 路由匹配 `/api`
- 客户端请求 `/api/users`
- `strip_prefix = "/api"`

则 upstream 看到的路径会变成 `/users`

### `proxy_set_headers`

允许为发往 upstream 的请求补充或覆盖自定义头。

## ACL 与限流

所有 handler 在真正执行前，都会先经过：

1. 访问控制
2. 限流

### 访问控制

字段：

- `allow_cidrs`
- `deny_cidrs`

规则：

- `deny` 优先
- 如果 `allow_cidrs` 为空，则表示默认允许
- 如果 `allow_cidrs` 非空，则客户端 IP 必须命中 allow

### 限流

模型：

- 基于客户端 IP
- 按 route 粒度隔离
- Token Bucket

字段：

- `requests_per_sec`
- `burst`

## 请求 ID

每个请求都会有 `X-Request-ID`：

- 客户端带了就复用
- 客户端没带就自动生成

它会贯穿：

- 下游响应头
- upstream 转发
- access log

## 推荐阅读

- [Configuration](Configuration.md)
- [Upstreams](Upstreams.md)
- [Operations](Operations.md)
