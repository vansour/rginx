# rginx 路线图、边界与实现进度

这份文档不是“想做什么就写什么”的愿望清单，而是当前 `rginx` 的能力矩阵、明确限制、工程演进观察，以及下一阶段的建议工作顺序。

如果你只想快速判断“这个能力现在能不能放心写进对外承诺”，请先看：

- [wiki/Release-Gate.md](wiki/Release-Gate.md)

如果你想判断“项目下一步最自然该做什么”，继续读本文即可。

## 如何阅读这份文档

状态说明：

- `✅ 已支持`
  - 代码已实现。
  - 仓库内有测试覆盖。
  - README / wiki 可以对外说明。
- `🚧 进行中`
  - 已有清晰实现基础，或已经完成一部分内部重构，但仍有明显补齐项。
- `📋 计划中`
  - 有明确价值，且与当前架构方向一致，但还未开始进入正式实现。
- `❌ 未支持 / 非目标`
  - 当前版本线不支持，或短期内不建议投入主要精力。

“已支持”不等于：

- 已与其他成熟入口代理完全语义兼容。
- 已经覆盖所有极端边界输入。
- 已在超大规模生产环境下完成长期验证。

`rginx` 当前的产品目标很明确：

- 面向中小规模部署的 Rust 入口反向代理。
- 先把 TLS 终止、Host/Path 路由、反向代理、基础静态文件、基础流量治理、健康检查、热重载和可观测性做扎实。
- 不追求在当前阶段成为其他入口代理的 drop-in replacement。

## 当前版本线

当前正式发布线收口为 `v0.1.1`。

这条版本线的工程重点不是继续无边界堆功能，而是：

1. 保持现有稳定能力与代码结构一致。
2. 降低文档、测试与实现之间的漂移。
3. 优先补真正影响上线可信度的协议和运维缺口。

## 能力矩阵

### 1. 配置与运行时

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| RON 顶层配置 | ✅ | `Config(runtime, server, upstreams, locations, servers)` 已稳定。 |
| 配置加载 | ✅ | 支持从显式路径或默认路径加载。 |
| 相对 `include` | ✅ | 支持 `// @include "relative/path.ron"` 文本级拼接。 |
| 字符串环境变量展开 | ✅ | 支持 `${VAR}` 与 `${VAR:-default}`。 |
| `rginx check` | ✅ | 会走完整加载、校验和编译链路。 |
| 配置语义校验 | ✅ | 字段合法性、跨字段约束、管理路由约束、gRPC 路由约束都已前置。 |
| 编译为运行时快照 | ✅ | 启动与重载都统一编译为 `ConfigSnapshot`。 |
| 动态配置 API | ✅ | `Config` handler 可读取当前生效配置，并通过 HTTP `PUT` 应用完整文档替换。 |
| 动态配置持久化 | ✅ | 动态配置更新会原子写回活跃配置文件。 |
| 动态配置 partial patch | ❌ | 当前只支持完整文档替换。 |
| 热重载 | ✅ | `SIGHUP` 支持无中断切换新快照。 |
| 热重载切换 `listen` | ❌ | 改监听地址必须重启。 |
| 热重载切换 `runtime.worker_threads` | ❌ | 变更 tokio worker 数必须重启。 |
| 热重载切换 `runtime.accept_workers` | ❌ | 变更 accept worker 数必须重启。 |

### 2. 入口监听、TLS 与连接层

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| HTTP/1.1 入站 | ✅ | 默认支持。 |
| HTTPS / TLS 终止 | ✅ | 支持 PEM 证书和私钥。 |
| SNI 多证书 | ✅ | 支持按 `server_names` 和 TLS 配置选择证书。 |
| 默认虚拟主机证书 | ✅ | 默认 vhost 可作为兜底证书来源。 |
| 入站 HTTP/2 | ✅ | 通过 TLS/ALPN 协商 `h2`。 |
| 明文入站 `h2c` | ❌ | 当前不支持。 |
| HTTP/1.1 keep-alive | ✅ | 可开关。 |
| 请求头数量限制 | ✅ | `server.max_headers`。 |
| 总并发连接数限制 | ✅ | `server.max_connections`。 |
| 请求头读取超时 | ✅ | `server.header_read_timeout_secs`。 |
| 响应写出超时 | ✅ | `server.response_write_timeout_secs`。 |
| 多监听地址 | ❌ | 当前单实例围绕一个 `listen` 工作。 |
| `SO_REUSEPORT` 多进程入口 | ❌ | 当前是单进程多 worker，不支持多进程端口复用架构。 |

### 3. 路由、虚拟主机与 handler

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| 默认虚拟主机 | ✅ | `server.server_names` 为空时可作为兜底主机。 |
| 额外虚拟主机列表 | ✅ | `servers: []`。 |
| 精确域名匹配 | ✅ | 例如 `api.example.com`。 |
| 通配符域名匹配 | ✅ | 例如 `*.example.com`。 |
| `Exact("/foo")` | ✅ | 优先级高于前缀。 |
| `Prefix("/api")` | ✅ | 按 segment boundary 匹配。 |
| 正则路由 | ❌ | 当前不支持。 |
| gRPC `service` 细分匹配 | ✅ | `grpc_service`。 |
| gRPC `method` 细分匹配 | ✅ | `grpc_method`。 |
| `Static` handler | ✅ | 支持状态码、内容类型和 body。 |
| `Proxy` handler | ✅ | 支持上游代理、header 改写、failover 等。 |
| `File` handler | ✅ | 支持文件、目录与 `try_files`。 |
| `Return` handler | ✅ | 支持状态码、Location 和可选响应体。 |
| `Status` handler | ✅ | 返回运行时 JSON 状态。 |
| `Metrics` handler | ✅ | 返回 Prometheus 文本格式指标。 |
| `Config` handler | ✅ | 受限的动态配置管理入口。 |
| 管理 handler 路由约束 | ✅ | `Config` handler 要求 `Exact(...)` 且必须配置非空 `allow_cidrs`。 |

### 4. 静态文件与内容服务

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| `root` | ✅ | 基础文件根目录。 |
| `index` | ✅ | 目录请求默认索引文件。 |
| `try_files` | ✅ | 顺序回退。 |
| `autoindex` | ✅ | 可选基础目录列表 HTML。 |
| `HEAD` | ✅ | 文件和目录列表都支持。 |
| 单段 `Range` | ✅ | 支持 `206 Partial Content`。 |
| 多段 `Range` | ❌ | 当前不支持 multipart ranges。 |
| 路径遍历保护 | ✅ | 会做路径安全校验与子路径约束。 |
| MIME 猜测 | ✅ | 内建基础类型映射。 |

### 5. 上游、代理与负载均衡

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| `http://` upstream | ✅ | 支持。 |
| `https://` upstream | ✅ | 支持。 |
| 上游 TLS 模式 | ✅ | `NativeRoots` / `CustomCa` / `Insecure`。 |
| `preserve_host` | ✅ | 可保留下游 Host。 |
| `strip_prefix` | ✅ | 基础 URL 改写。 |
| `proxy_set_headers` | ✅ | 支持显式补充或覆盖请求头。 |
| 连接池 | ✅ | 支持 idle timeout 和 per-host idle 上限。 |
| 上游 connect/read/write/idle timeout | ✅ | 已支持细粒度超时控制。 |
| TCP keepalive / nodelay | ✅ | 已支持。 |
| HTTP/2 keepalive 调优 | ✅ | upstream 方向已支持 interval / timeout / while idle。 |
| `round_robin` | ✅ | 默认策略。 |
| `ip_hash` | ✅ | 基于解析后的真实客户端 IP。 |
| `least_conn` | ✅ | 按活动请求数选择。 |
| `weight` | ✅ | 适用于当前三种策略。 |
| `backup` peer | ✅ | 主节点不可用时接管。 |
| 幂等 / 可重放请求 failover | ✅ | 只在请求体可安全重放时自动重试其他 peer。 |
| 被动健康检查 | ✅ | 失败计数 + 冷却窗口。 |
| 主动 HTTP 健康检查 | ✅ | 定时探测。 |
| 主动 gRPC health check | ✅ | 支持标准 health service，但当前要求 `https://` peer。 |
| 明文 upstream `h2c` | ❌ | 当前不支持。 |
| Proxy Protocol | ❌ | 当前不支持。 |

### 6. HTTP/2、gRPC 与 grpc-web

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| 入站 HTTP/2 | ✅ | TLS/ALPN。 |
| 上游 HTTP/2 | ✅ | `https://` + TLS/ALPN，`protocol = Http2` 可强制要求。 |
| gRPC over HTTP/2 代理 | ✅ | 支持基础请求/响应链路。 |
| 请求 trailers 透传 | ✅ | 支持。 |
| 响应 trailers 透传 | ✅ | 支持。 |
| grpc-web binary | ✅ | 基础转换已支持。 |
| grpc-web text | ✅ | 基础转换已支持。 |
| grpc-web request trailer frame | ✅ | 可转换到 upstream HTTP/2 request trailers。 |
| grpc-web response trailer frame | ✅ | upstream trailers 可重新编码返回。 |
| `grpc-timeout` deadline | ✅ | 取下游 deadline 与 upstream request timeout 的较小值。 |
| 本地代理错误映射到 `grpc-status` | ✅ | 未命中、ACL、限流、upstream 错误、超时都会尽量做 gRPC 风格返回。 |
| 下游提前取消记账 | ✅ | 若未观察到最终 `grpc-status`，会补记为 `grpc-status = 1`。 |
| gRPC route selection | ✅ | 支持 `grpc_service` / `grpc_method`。 |
| 明文 gRPC upstream `h2c` | ❌ | 当前不支持。 |
| 更完整高级 gRPC 语义 | ❌ | 例如更主动的 cancellation 协同、更完整协议级兼容性，当前未承诺。 |
| HTTP/2 extended CONNECT | ❌ | 当前不支持。 |

### 7. 流量治理与客户端识别

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| `trusted_proxies` | ✅ | 支持 IP / CIDR。 |
| `X-Forwarded-For` 真实 IP 解析 | ✅ | 只在对端属于 trusted proxy 时启用。 |
| 路由级 CIDR allow / deny | ✅ | 已支持。 |
| 路由级限流 | ✅ | 基于客户端 IP 的 token bucket。 |
| 请求体大小限制 | ✅ | `server.max_request_body_bytes`。 |
| 慢请求头 / 慢请求体治理 | ✅ | `header_read_timeout_secs` / `request_body_read_timeout_secs`。 |
| 慢客户端写出治理 | ✅ | `response_write_timeout_secs`。 |

### 8. 可观测性、运维与发布

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| 默认 access log | ✅ | tracing 结构化输出。 |
| 自定义 access log 模板 | ✅ | `server.access_log_format`。 |
| `X-Request-ID` 透传或生成 | ✅ | 已贯通请求链路、响应和 access log。 |
| `/status` JSON | ✅ | 含 revision、route、upstream、peer health 等摘要。 |
| `/metrics` Prometheus | ✅ | HTTP、gRPC、reload、health check、upstream 等基础指标齐全。 |
| 配置重载指标 | ✅ | success / failure 计数。 |
| `Ctrl-C` / `SIGTERM` 平滑退出 | ✅ | 支持。 |
| `SIGHUP` 热重载 | ✅ | 支持。 |
| 安装 / 卸载脚本 | ✅ | 仓库内已提供。 |
| Release workflow | ✅ | 已有准备脚本和 release 文档。 |
| 内建 systemd / service unit | ❌ | 当前不内置。 |
| OpenTelemetry | ❌ | 当前不支持。 |

### 9. 工程化与测试

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| crate 分层 | ✅ | `app / config / core / http / runtime / observability` 职责明确。 |
| 配置加载 / 校验 / 编译分层 | ✅ | 已拆清。 |
| `handler/` 目录化 | ✅ | 已从单大文件拆成 `dispatch / admin / grpc / access_log`。 |
| `proxy/` 目录化 | ✅ | 已从单大文件拆成 `clients / forward / health / grpc_web / request_body / upgrade`。 |
| 集成测试共享 harness | ✅ | `tests/support/mod.rs` 已统一 child 管理、日志和 ready probe。 |
| 全工作区回归 | ✅ | 当前可通过 `cargo test --workspace`。 |
| 更细粒度模块继续拆分 | 🚧 | `rginx-core/config.rs`、`rginx-config/compile.rs`、`rginx-config/validate.rs` 仍偏大，但职责已相对集中。 |

## 当前明确限制与非目标

下面这些项当前不应该被当作 `v0.1.1` 的稳定承诺：

### 协议边界

- 不支持明文入站 HTTP/2（`h2c`）。
- 不支持明文 upstream HTTP/2（`h2c`）。
- 不支持明文 `h2c` gRPC upstream。
- 不支持 HTTP/2 extended CONNECT。

### 路由与配置边界

- 不支持正则路由。
- 动态配置 API 只支持完整文档替换，不支持 partial patch。
- 热重载不能切换 `listen`、`runtime.worker_threads` 或 `runtime.accept_workers`。
- 单实例当前围绕一个监听地址工作，不是多 `listen` 入口模型。

### 代理与兼容性边界

- 只承诺基础 grpc-web binary / text 模式，不承诺更完整高级 grpc-web 兼容。
- 不支持 Proxy Protocol。
- 当前产品目标不是其他成熟入口代理的全量语义兼容实现。

### 运维与发布边界

- 默认假设前面如果存在 LB / CDN / 其他代理，部署者会正确配置 `trusted_proxies`。
- 默认假设 `/status`、`/metrics` 和任何 `Config` 管理路由都放在受控网络里。
- 默认假设进程生命周期由外部 supervisor 管理，而不是由 `rginx` 自己充当完整服务管理器。

## 工程演进观察

当前代码结构里，已经完成了几件真正有价值的内部整理：

### 已完成的结构收口

- 配置链路已经稳定分成 `load -> validate -> compile` 三层。
- 请求处理主入口已经收口到 `handler/dispatch.rs`，而不是把所有 handler 逻辑混在一个文件里。
- proxy 主逻辑已经按“client / request body / forward / health / grpc-web / upgrade”拆开。
- 集成测试里重复的 child 启动、ready 检查和日志收集已经提取到共享 harness。

### 仍然值得持续关注的大文件

- [`crates/rginx-core/src/config.rs`](crates/rginx-core/src/config.rs)
- [`crates/rginx-config/src/compile.rs`](crates/rginx-config/src/compile.rs)
- [`crates/rginx-config/src/validate.rs`](crates/rginx-config/src/validate.rs)
- [`crates/rginx-http/src/proxy/health.rs`](crates/rginx-http/src/proxy/health.rs)

这些文件当前的问题不是职责混乱，而是体量大、领域细节密度高。下一步如果继续拆，最自然的方向不是“为了拆而拆”，而是按子领域边界拆：

- 配置编译：`runtime / server / upstream / route / vhost`
- 配置校验：`runtime / server / upstream / route / vhost`
- 核心模型：`route / access_log / upstream`
- proxy health：`registry / probe / grpc_health_codec`

## 建议的下一阶段路线

下面的阶段不是对外版本承诺，而是按当前代码形态最自然、最值得投入的建议顺序。

### Phase A：发布线硬化与文档收口

状态：`🚧`

目标：

- 保证 README、ROADMAP、wiki、测试和代码行为不再明显漂移。
- 继续把集成测试脚手架统一化，降低 flaky 行为。

建议工作：

- 维护一份更稳定的能力矩阵和限制说明。
- 给动态配置、reload、gRPC、TLS、active health check 补更多端到端用例。
- 让新特性默认同时带上 README / ROADMAP / wiki 更新。

完成标志：

- 常见能力都能在 README、ROADMAP、wiki 中找到一致表述。
- 工作区回归保持稳定，没有明显测试竞态热点。

### Phase B：更完整的 gRPC / grpc-web 语义

状态：`📋`

为什么优先：

- 这是当前协议面最复杂、最容易在真实生产流量里遇到边界行为的部分。
- 现有代码已经有不错的基础：deadline、trailers、grpc-web、错误映射、取消记账都已具备。

建议工作：

- 更主动的 downstream cancellation 与 upstream 交互协同。
- 更细粒度的 gRPC / grpc-web 错误分类与 observability。
- 继续增加协议级端到端测试覆盖，特别是 body streaming、trailers、timeout 和 early cancel 组合。

完成标志：

- gRPC 行为在“正常返回 / trailer / timeout / cancel / local error”五类路径下都更可解释、更稳定。

### Phase C：更灵活的入口匹配与协议覆盖

状态：`📋`

建议工作：

- 正则路由。
- 是否要引入明文 `h2c` 的评估与最小实现。
- 是否要引入 Proxy Protocol 的评估与最小实现。

排序建议：

1. 如果部署现实里前面经常接 LB / CDN，再考虑 Proxy Protocol。
2. 如果 gRPC 与 mesh 场景变多，再评估 `h2c`。
3. 正则路由要做，但应晚于前两个“更靠近生产风险”的能力。

### Phase D：配置管理与运维增强

状态：`📋`

建议工作：

- 更丰富的 `/status` 和管理接口字段。
- 动态配置 API 的 patch / staged update 能力评估。
- 更清晰的运维隔离建议与示例。
- 安装、卸载、发布与服务托管文档进一步收口。

### Phase E：继续按子领域细化内部模块

状态：`📋`

目标：

- 进一步降低大文件维护成本。
- 让后续功能改动的落点更稳定。

建议原则：

- 只按清晰子领域拆，不做表面文件切碎。
- 拆分时同步保留集成测试与文档更新。
- 避免再次制造“README 说的是一套，代码目录已经变另一套”的文档滞后。

## 不建议优先投入的方向

当前阶段，不建议把主要精力放在这些方向上：

- 大量为了兼容其他代理配置语法而引入的语法糖。
- 未配测试、未配文档的细碎行为模仿。
- 没有真实部署场景驱动的复杂负载均衡变体。

更稳妥的顺序应该是：

1. 先修真实协议与运维风险。
2. 再补配置与管理体验。
3. 最后做更广泛的兼容性与语法扩展。

## 文档与测试同步约定

以后新增或调整能力时，建议把下面几项当成一套动作，而不是只改代码：

1. 补或更新单元测试 / 集成测试。
2. 更新 README 的能力描述或示例。
3. 更新本文件里的能力矩阵、限制或阶段规划。
4. 更新对应 wiki 页面。

如果四者不同步，后续维护成本会上升得非常快。

## 参考

- 顶层说明：[README.md](README.md)
- 稳定承诺与发布闸门：[wiki/Release-Gate.md](wiki/Release-Gate.md)
- 架构总览：[wiki/Architecture.md](wiki/Architecture.md)
- 开发说明：[wiki/Development.md](wiki/Development.md)
- wiki 摘要版路线图：[wiki/Roadmap-and-Gaps.md](wiki/Roadmap-and-Gaps.md)
