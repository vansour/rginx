# Capability Matrix

本页给出当前版本线最适合直接引用的“能力矩阵摘要”：

- 哪些能力已经进入当前稳定支持范围
- 这些能力主要由哪些测试文件覆盖
- 哪些边界需要看更细的文档说明

如果你需要：

- 看更细的逐项能力表和非目标边界：
  - [ROADMAP.md](../ROADMAP.md)
- 看当前正式发布线的稳定承诺和发布闸门：
  - [Release Gate](Release-Gate.md)

## 如何阅读

这里的“稳定支持”采用和发布文档一致的标准：

- 代码已实现
- 仓库内有测试覆盖
- README / wiki 可以对外说明

本页不重复展开每个字段级约束，只给出适合快速判断的能力域摘要和测试落点。

## 当前稳定支持摘要

| 能力域 | 当前状态 | 主要验证来源 | 说明 |
| --- | --- | --- | --- |
| 配置加载、校验与编译 | ✅ 稳定支持 | `crates/rginx-config` 单元测试、`crates/rginx-app/tests/check.rs` | 覆盖 `include`、环境变量展开、配置校验、运行时快照编译、`rginx check`。 |
| 热重载与动态配置 API | ✅ 稳定支持 | [`reload.rs`](../crates/rginx-app/tests/reload.rs)、[`dynamic_config_api.rs`](../crates/rginx-app/tests/dynamic_config_api.rs) | 支持整份配置替换；`listen`、`runtime.worker_threads`、`runtime.accept_workers` 仍然不能在线变更。 |
| 默认虚拟主机与额外虚拟主机 | ✅ 稳定支持 | [`vhost.rs`](../crates/rginx-app/tests/vhost.rs)、[`check.rs`](../crates/rginx-app/tests/check.rs) | 覆盖精确域名、通配符域名和“不回落到 default vhost routes”的语义。 |
| `Static` / `File` / `Return` / `Status` / `Metrics` / `Config` handler | ✅ 稳定支持 | [`phase1.rs`](../crates/rginx-app/tests/phase1.rs)、[`dynamic_config_api.rs`](../crates/rginx-app/tests/dynamic_config_api.rs)、[`active_health.rs`](../crates/rginx-app/tests/active_health.rs) | 覆盖基础 handler 路径、状态页、指标页和配置管理接口。 |
| 静态文件、`HEAD`、单段 `Range`、`autoindex` | ✅ 稳定支持 | [`phase1.rs`](../crates/rginx-app/tests/phase1.rs) | 当前只承诺基础目录列表和单段 `Range`；不支持 multipart ranges。 |
| 上游代理与基础 header 改写 | ✅ 稳定支持 | [`phase1.rs`](../crates/rginx-app/tests/phase1.rs)、`rginx-http` proxy 单元测试 | 覆盖 request id 透传、基础代理链路、header 清洗与 URI 改写。 |
| `round_robin` / `weight` / `backup` / failover | ✅ 稳定支持 | [`weighted_round_robin.rs`](../crates/rginx-app/tests/weighted_round_robin.rs)、[`backup.rs`](../crates/rginx-app/tests/backup.rs)、[`failover.rs`](../crates/rginx-app/tests/failover.rs) | failover 只在幂等或可重放请求上自动发生。 |
| `ip_hash` / `least_conn` | ✅ 稳定支持 | [`ip_hash.rs`](../crates/rginx-app/tests/ip_hash.rs)、[`least_conn.rs`](../crates/rginx-app/tests/least_conn.rs) | `ip_hash` 基于解析后的真实客户端 IP。 |
| 被动健康检查与主动健康检查 | ✅ 稳定支持 | [`active_health.rs`](../crates/rginx-app/tests/active_health.rs)、[`grpc_proxy.rs`](../crates/rginx-app/tests/grpc_proxy.rs) | 覆盖 HTTP 主动探测、标准 gRPC health service 探测、摘除与恢复。 |
| TLS、SNI、入站 HTTP/2、上游 HTTP/2 | ✅ 稳定支持 | [`http2.rs`](../crates/rginx-app/tests/http2.rs)、[`upstream_http2.rs`](../crates/rginx-app/tests/upstream_http2.rs)、`rginx-http` TLS 单元测试 | 入站 HTTP/2 走 TLS/ALPN；上游 HTTP/2 当前要求 `https://` peer。 |
| gRPC over HTTP/2 与 grpc-web binary/text | ✅ 稳定支持 | [`grpc_proxy.rs`](../crates/rginx-app/tests/grpc_proxy.rs) | 覆盖 trailers、grpc-web 转换、`grpc-timeout`、取消传播、路由细分和错误映射。 |
| ACL、限流与真实客户端 IP | ✅ 稳定支持 | [`policy.rs`](../crates/rginx-app/tests/policy.rs)、[`ip_hash.rs`](../crates/rginx-app/tests/ip_hash.rs)、`rginx-http` 单元测试 | 覆盖 `allow_cidrs` / `deny_cidrs`、基于客户端 IP 的限流和 `trusted_proxies`。 |
| 压缩、连接与请求硬化 | ✅ 稳定支持 | [`compression.rs`](../crates/rginx-app/tests/compression.rs)、[`hardening.rs`](../crates/rginx-app/tests/hardening.rs) | 覆盖 br/gzip 协商、请求头限制、请求体限制、慢请求和连接上限。 |
| Upgrade / WebSocket 透传 | ✅ 稳定支持 | [`upgrade.rs`](../crates/rginx-app/tests/upgrade.rs) | 当前覆盖基础 HTTP/1.1 Upgrade 隧道。 |
| access log、request id、状态与指标 | ✅ 稳定支持 | [`access_log.rs`](../crates/rginx-app/tests/access_log.rs)、[`phase1.rs`](../crates/rginx-app/tests/phase1.rs)、[`active_health.rs`](../crates/rginx-app/tests/active_health.rs)、[`grpc_proxy.rs`](../crates/rginx-app/tests/grpc_proxy.rs) | 覆盖自定义 access log 模板、`X-Request-ID`、状态页和 Prometheus 指标。 |
| 单进程多 worker 运行时 | ✅ 稳定支持 | [`workers.rs`](../crates/rginx-app/tests/workers.rs) | 当前产品模型是单进程多 worker，不是多进程端口复用。 |

## 当前明确不在稳定承诺内

下面这些点当前仍应按“未支持 / 非目标”理解：

- 明文入站 HTTP/2（`h2c`）
- 明文 upstream HTTP/2（`h2c`）
- 明文 `h2c` gRPC upstream
- 正则路由
- 动态配置 API partial patch
- 热重载切换 `listen`
- 热重载切换 `runtime.worker_threads`
- 热重载切换 `runtime.accept_workers`
- Proxy Protocol
- `SO_REUSEPORT` 多进程 worker 架构
- multipart `Range`
- 更完整的高级 grpc-web / gRPC 兼容语义
- 更通用的流式压缩策略

更完整的边界说明见：

- [Release Gate](Release-Gate.md)
- [ROADMAP.md](../ROADMAP.md)

## 当前建议的文档优先级

如果你在做能力判断，建议按下面顺序看：

1. 代码与测试行为
2. [Release Gate](Release-Gate.md)
3. 本页
4. [ROADMAP.md](../ROADMAP.md)
5. README 与其他 wiki 页面
