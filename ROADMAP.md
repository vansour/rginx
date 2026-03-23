# Rginx 功能边界与实现进度

本文档用于追踪 rginx 在“面向中小规模部署的 Rust 入口反向代理”这一产品范围内，哪些能力已经稳定支持，哪些能力仍待补齐。

状态说明：
- ✅ **已支持**: 功能已实现并有测试覆盖。
- 🚧 **进行中**: 正在开发或已有基础实现但需增强。
- 📋 **计划中**: 已列入路线图，尚未开始。
- ❌ **未支持**: 尚未列入近期计划或明确不支持。

---

## 1. 核心架构与配置

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **单进程 / 多 Worker** | ❌ | 当前以单进程运行；后续再评估多 worker / `SO_REUSEPORT` 形态。 |
| **配置热加载** | ✅ | 支持 SIGHUP 平滑加载，现有连接不中断。 |
| **配置文件 Include** | ❌ | 暂不支持拆分配置文件。 |
| **动态配置 / API** | ❌ | 无运行时 API 修改配置（P3）。 |
| **配置校验** | ✅ | 支持 `rginx check` 命令。 |
| **环境变量支持** | ❌ | 配置中暂不支持读取环境变量。 |

## 2. Server / 监听

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **HTTP/1.1 监听** | ✅ | 支持。 |
| **HTTPS/TLS 监听** | ✅ | 支持 PEM 格式证书/密钥。 |
| **HTTP/2 (h2)** | ✅ | 支持入站 TLS/ALPN 协商到 HTTP/2；明文 h2c 暂不支持。 |
| **Server Name (SNI)** | ✅ | 支持多证书 SNI，基于 Host 头路由到不同 VirtualHost。 |
| **监听端口复用** | ❌ | 当前一个端口对应一个配置实例。 |
| **listen ... default_server** | ❌ | 依赖 Server Name 功能实现。 |
| **SO_REUSEPORT** | ❌ | 依赖多 Worker 架构。 |

## 3. 路由与匹配

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **精确匹配 (=)** | ✅ | `MatcherConfig::Exact`。 |
| **前缀匹配 (^~ / 无修饰)** | ✅ | `MatcherConfig::Prefix`。 |
| **正则匹配 (~ ~\*)** | ❌ | 暂不支持。建议优先级低于前缀改写。 |
| **Host 匹配** | ✅ | 支持 VirtualHost 概念，支持通配符域名 (`*.example.com`)。 |
| **try_files** | ✅ | 静态文件服务支持 `try_files` 回退列表。 |

## 4. 代理

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **HTTP 反向代理** | ✅ | 基础功能完备。 |
| **Websocket 透传** | ✅ | 支持 HTTP/1.1 `Connection: Upgrade` / WebSocket 透传；暂不含 HTTP/2 extended CONNECT。 |
| **上游 HTTP/2** | ✅ | 支持 `https://` upstream 通过 TLS/ALPN 自动协商到 HTTP/2，也可通过 `upstream.protocol = Http2` 强制要求；上游明文 `h2c` 暂不支持。 |
| **gRPC 代理** | ❌ | 上游 h2 基础层已具备，但 trailer / streaming 等 gRPC 语义尚未补齐。 |
| **Proxy Protocol** | ❌ | 暂不支持。 |
| **上游 TLS** | ✅ | 支持系统根证书、自定义 CA、Insecure。 |
| **上游连接池** | ✅ | 支持 idle timeout 和每 host idle 连接数上限。 |

### 4.1 请求/响应处理

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **X-Forwarded-For** | ✅ | 自动追加，支持 Trusted Proxies。 |
| **X-Forwarded-Proto** | ✅ | 自动添加。 |
| **X-Forwarded-Host** | ✅ | 自动添加。 |
| **Host 头改写** | ✅ | 支持 `preserve_host` 开关，默认改为上游 Authority。 |
| **URL Rewrite** | ✅ | 支持 `strip_prefix` 移除路径前缀。 |
| **重定向** | ✅ | 支持 `return` 指令，支持 301/302/307/308 等状态码。 |
| **自定义 Header** | ✅ | 支持 `proxy_set_headers` 添加自定义请求头。 |

## 5. 负载均衡

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **Round Robin** | ✅ | 默认策略。 |
| **Weight** | ✅ | 支持 `peer.weight`，适用于 `round_robin`、`ip_hash`、`least_conn`。 |
| **Ip Hash** | ✅ | 支持基于解析后客户端 IP 的稳定 peer 选择，并在 peer 不健康时顺序回退。 |
| **Least Conn** | ✅ | 支持按 peer 当前活跃请求数选择最空闲节点；活跃数相同时按配置顺序选择。 |
| **被动健康检查** | ✅ | 支持失败计数与冷却。 |
| **主动健康检查** | ✅ | 支持定期 HTTP 探测。 |
| **Backup Peer** | ✅ | 支持 `peer.backup`，仅在主 peer 不可用时接管流量。 |

## 6. 流量治理

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **限流** | ✅ | 支持基于 IP 的 Token Bucket。 |
| **IP 黑白名单** | ✅ | 支持 CIDR allow/deny。 |
| **最大连接数** | ✅ | Server 级别限制。 |
| **请求体大小限制** | ✅ | `max_request_body_bytes`。 |
| **超时控制** | 🚧 | 已支持 `header_read_timeout` 和 upstream `connect/read/write/idle` 精细化配置；客户端侧超时仍待补齐。 |

## 7. 内容服务

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **静态文件** | ✅ | 支持 `root`、`try_files`、MIME 类型、`HEAD`、单段 `Range`。 |
| **Index** | ✅ | 支持 `index` 指令指定默认索引文件。 |
| **Autoindex** | ❌ | 暂不支持目录列表。 |
| **Gzip/Brotli** | ❌ | 计划中。 |

## 8. 可观测性

| 能力项 | Rginx 状态 | 说明 / 差异 |
| :--- | :--- | :--- |
| **Access Log** | 🚧 | 通过 Tracing 输出结构化字段，已包含 `request_id` / `host` / `vhost` / `route`，但仍缺自定义格式。 |
| **Error Log** | ✅ | Tracing 实现。 |
| **Prometheus Metrics** | ✅ | 基础指标齐全。 |
| **Status Page** | ✅ | 提供 JSON 状态接口。 |
| **请求 ID** | ✅ | 支持复用或自动生成 `X-Request-ID`，并贯通下游响应、upstream 转发和 access log。 |

---

## 近期冲刺建议

根据对标表，建议按以下顺序填补差距：

1.  **S1: 多虚拟主机支持** ✅ 已完成
    *   目标：支持 `server_name` 匹配，允许单实例托管多域名。
    *   关键改动：重构 `Config` 模型，引入 `Server` 列表，修改 Router 匹配逻辑。

2.  **S2: 代理行为增强** ✅ 已完成
    *   目标：完善 Header 处理与 URL 改写。
    *   关键改动：增加 `preserve_host`, `strip_prefix`, `proxy_set_headers` 配置项。

3.  **S3: 静态文件服务** ✅ 已完成
    *   目标：支持 `root` 指令，作为 Web Server 托管前端静态资源。
    *   关键改动：新增 `HandlerConfig::File`，实现文件读取与 MIME 判断。

4.  **S4: 重定向功能** ✅ 已完成
    *   目标：支持 `return` 指令配置 HTTP 重定向。
    *   关键改动：新增 `HandlerConfig::Return`，支持 301/302/307/308 状态码和自定义响应。
