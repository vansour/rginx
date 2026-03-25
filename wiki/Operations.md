# Operations

本页面向运维和上线场景，覆盖配置检查、热重载、状态页、指标和日志。

## 压缩

当前支持基础 br/gzip 响应压缩：

- 只在客户端发送 `Accept-Encoding: br` 或 `Accept-Encoding: gzip` 时启用
- 会按 `q` 值协商 `br` / `gzip`，同分时优先 `br`
- 主要面向小体积文本类响应
- 会跳过 `206 Partial Content`、`Range`、gRPC、已有 `Content-Encoding` 的响应

当前仍不支持：

- 面向所有流式 upstream 响应的通用压缩

## 常用命令

检查配置：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
```

启动服务：

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

构建后运行：

```bash
cargo build -p rginx
./target/debug/rginx --config configs/rginx.ron
```

## Runtime Worker

当前支持单进程多 worker 运行时：

- `runtime.worker_threads` 控制 tokio runtime worker 线程数
- `runtime.accept_workers` 控制监听 socket 的 accept worker 数

当前不支持：

- `SO_REUSEPORT` 多进程 worker 形态

## 动态配置 API

如果你显式配置了 `Config` 管理路由，就可以通过 HTTP 读取当前生效配置，或用完整 RON 文档在线替换配置。

最小示例：

```ron
LocationConfig(
    matcher: Exact("/-/config"),
    handler: Config,
    allow_cidrs: ["127.0.0.1/32", "::1/128"],
)
```

读取当前生效配置：

```bash
curl http://127.0.0.1:8080/-/config
```

应用新配置：

```bash
curl -X PUT \
  -H 'Content-Type: application/ron; charset=utf-8' \
  --data-binary @configs/rginx.ron \
  http://127.0.0.1:8080/-/config
```

边界：

- `Config` 路由必须是 `Exact(...)`，并且必须配置非空 `allow_cidrs`
- `PUT` body 必须是非空、有效 UTF-8 的完整 RON 文档
- `PUT` body 当前限制为 1 MiB；超限会返回 `413 Payload Too Large`
- 当前只支持完整文档替换，不支持 partial patch
- 新配置会先通过 validate + compile，再落盘并切换到新的运行时快照
- `listen`、`runtime.worker_threads`、`runtime.accept_workers` 仍然不能在线变更

## 日志

日志通过 `tracing` 输出，默认级别大致是：

- `info`
- `rginx_core=info`
- `rginx_http=info`

可以用 `RUST_LOG` 覆盖：

```bash
RUST_LOG=debug,rginx_http=trace ./target/debug/rginx --config configs/rginx.ron
```

默认 access log 是结构化 tracing 事件，字段当前已包含：

- `request_id`
- `method`
- `host`
- `path`
- `client_ip`
- `client_ip_source`
- `peer_addr`
- `vhost`
- `route`
- `status`
- `grpc_protocol`
- `grpc_service`
- `grpc_method`
- `grpc_status`
- `grpc_message`
- `elapsed_ms`

其中：

- `grpc_status` / `grpc_message` 会优先取最终 gRPC trailers
- 对 `grpc-web` / `grpc-web-text`，会从最终 trailer frame 中提取同名字段
- 如果没有 trailers，再回退读取响应头中的 `grpc-status` / `grpc-message`
- 如果下游在响应流结束前提前取消，且代理尚未观察到最终 `grpc-status`，则会补记 `grpc_status = 1`

如果你想要更接近传统反向代理的单行日志，可以在 `server.access_log_format` 里定义模板：

```ron
ServerConfig(
    listen: "0.0.0.0:8080",
    access_log_format: Some("reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method status=$status grpc_status=$grpc_status request=\"$request\" bytes=$body_bytes_sent elapsed=$request_time_ms"),
)
```

当前支持的变量包括：

- `$request_id`
- `$remote_addr`
- `$peer_addr`
- `$method`
- `$host`
- `$path`
- `$request`
- `$status`
- `$body_bytes_sent`
- `$request_time_ms`
- `$client_ip_source`
- `$vhost`
- `$route`
- `$scheme`
- `$http_version`
- `$http_user_agent`
- `$http_referer`
- `$grpc_protocol`
- `$grpc_service`
- `$grpc_method`
- `$grpc_status`
- `$grpc_message`

## 热重载

Unix 上发送：

```bash
kill -HUP <pid>
```

热重载行为：

- 重新读取配置文件
- 重新执行 validate + compile
- 用新的 `ConfigSnapshot` 替换运行时状态
- 现有连接不中断

限制：

- `listen` 地址不能变
- `runtime.worker_threads` 不能变
- `runtime.accept_workers` 不能变
- 如果你需要改监听地址，请重启进程

## 平滑退出

支持：

- `Ctrl-C`
- `SIGTERM`

退出过程：

1. 停止 accept 新连接
2. 通知健康检查任务和连接处理逻辑进入收敛
3. 等待 `shutdown_timeout_secs`
4. 超时后 abort 尚未退出的后台任务

## `/status`

典型用于：

- 内网巡检
- 容器 sidecar 检查
- 临时排障

建议只对内网开放，通常配合：

```ron
LocationConfig(
    matcher: Exact("/status"),
    handler: Status,
    allow_cidrs: ["127.0.0.1/32", "::1/128"],
),
```

当前状态页会返回：

- `revision`
- `listen`
- `tls_enabled`
- `keep_alive`
- `max_connections`
- `trusted_proxy_count`
- `active_connections`
- `vhost_count`
- `route_count`
- `upstream_count`
- 每个 upstream 的：
  - `peer_count`
  - `healthy_peer_count`
  - `backup_peer_count`
  - `active_requests`
  - transport / pool / health 配置
- 每个 peer 的：
  - `weight`
  - `backup`
  - `healthy`
  - `active_requests`
  - 被动失败与冷却信息
  - 主动健康状态

说明：

- `active_connections` 表示当前进程持有的活跃客户端连接数，适合快速判断连接压力和命中 `max_connections` 的风险
- `trusted_proxy_count` 只反映配置里受信 CIDR 条目数，不代表实时代理层数
- `read_timeout_ms` 当前为了兼容已有状态页字段，仍等同于 upstream 的整体 `request_timeout_ms`，并不是单独的 socket read timeout

## `/metrics`

Prometheus 指标当前包括：

- `rginx_active_connections`
- `rginx_http_requests_total`
- `rginx_grpc_requests_total`
- `rginx_grpc_responses_total`
- `rginx_http_rate_limited_total`
- `rginx_http_request_duration_ms`
- `rginx_upstream_requests_total`
- `rginx_active_health_checks_total`
- `rginx_config_reloads_total`

指标维度约定：

- 优先保持 metric name 稳定，不轻易重命名已有指标
- label 只放低基数、便于聚合的维度，比如 `route`、`status`、`protocol`、`service`、`method`、`upstream`、`peer`、`result`
- 不要把 `request_id`、客户端 IP、原始 `host`、原始 path、User-Agent 等高基数字段放进 label；这些信息应放到 access log

建议只开放给：

- Prometheus
- Grafana Agent / Alloy
- 内网抓取器

## 真实客户端 IP

如果 `rginx` 前面还有一层代理、LB 或 CDN，需要配置 `trusted_proxies`。

流程：

1. 如果 TCP 对端不在 `trusted_proxies` 内，直接使用对端 IP。
2. 如果对端受信，则解析 `X-Forwarded-For`。
3. 从右向左选择最后一个非受信代理 IP 作为真实客户端地址。

这会影响：

- access log
- ACL
- 限流
- `ip_hash`

## 生产建议

上线前建议逐项检查：

1. `rginx check` 已通过
2. `/status`、`/metrics` 和任何 `Config` 管理路由都已限制访问范围
3. `trusted_proxies` 配置准确
4. upstream timeout 和 server timeout 合理
5. 主动健康检查路径稳定可用
6. 至少跑过一次热重载演练
7. 至少跑过一次平滑退出演练

如果你在准备正式版发布，或者要判断某项能力是否已经进入稳定承诺，请再对照：

- [Release Gate](Release-Gate.md)

## 典型排障思路

### 请求都打到 backup

优先看：

- `/status` 里主 peer 是否不健康
- `unhealthy_after_failures` 是否过小
- 主动健康检查路径是否错误
- `healthy_peer_count` 是否已经降到只剩 backup 可用

### `ip_hash` 看起来不稳定

优先看：

- `trusted_proxies` 是否缺失
- 前置代理是否正确传了 `X-Forwarded-For`

### 热重载失败

优先看：

- 新配置是否能单独通过 `rginx check`
- 是否修改了监听地址
- TLS 文件或 CA 文件路径是否错误

### 请求量正常但延迟突然升高

优先看：

- `/metrics` 里的 `rginx_http_request_duration_ms`
- `/status` 里的 `active_connections` 和 upstream `active_requests`
- access log 里的 `route`、`status`、`elapsed_ms`

## 推荐阅读

- [Upstreams](Upstreams.md)
- [TLS and HTTP2](TLS-and-HTTP2.md)
- [Deployment and Service Hosting](Deployment-and-Service-Hosting.md)
- [Examples](Examples.md)
- [Release Gate](Release-Gate.md)
