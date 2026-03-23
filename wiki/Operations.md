# Operations

本页面向运维和上线场景，覆盖配置检查、热重载、状态页、指标和日志。

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

## 日志

日志通过 `tracing` 输出，默认级别大致是：

- `info`
- `rginx_core=info`
- `rginx_http=info`

可以用 `RUST_LOG` 覆盖：

```bash
RUST_LOG=debug,rginx_http=trace ./target/debug/rginx --config configs/rginx.ron
```

访问日志字段当前已包含：

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
- `elapsed_ms`

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
- `vhost_count`
- `route_count`
- `upstream_count`
- 每个 upstream 的 transport / pool / health 配置
- 每个 peer 的：
  - `weight`
  - `backup`
  - `healthy`
  - `active_requests`
  - 被动失败与冷却信息
  - 主动健康状态

## `/metrics`

Prometheus 指标当前包括：

- `rginx_active_connections`
- `rginx_http_requests_total`
- `rginx_http_rate_limited_total`
- `rginx_http_request_duration_ms`
- `rginx_upstream_requests_total`
- `rginx_active_health_checks_total`
- `rginx_config_reloads_total`

建议只开放给：

- Prometheus
- Grafana Agent / Alloy
- 内网抓取器

## 真实客户端 IP

如果 `Rginx` 前面还有一层代理、LB 或 CDN，需要配置 `trusted_proxies`。

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
2. `/status` 和 `/metrics` 已限制访问范围
3. `trusted_proxies` 配置准确
4. upstream timeout 合理
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

### `ip_hash` 看起来不稳定

优先看：

- `trusted_proxies` 是否缺失
- 前置代理是否正确传了 `X-Forwarded-For`

### 热重载失败

优先看：

- 新配置是否能单独通过 `rginx check`
- 是否修改了监听地址
- TLS 文件或 CA 文件路径是否错误

## 推荐阅读

- [Upstreams](Upstreams.md)
- [TLS and HTTP2](TLS-and-HTTP2.md)
- [Examples](Examples.md)
- [Release Gate](Release-Gate.md)
