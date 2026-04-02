# rginx 路线图、边界与实现进度

这份文档不是愿望清单，而是当前 `rginx` 的能力矩阵、明确限制、工程演进观察，以及围绕“纯 Rust 反向代理”目标的阶段性改造计划。

如果你只想判断“这个能力现在能不能写进对外承诺”，先看本文的“能力矩阵”和“当前明确限制与非目标”。

如果你想判断“项目下一步应该按什么顺序改”，看本文的“纯反向代理化分阶段计划”。

## 如何阅读这份文档

状态说明：

- `✅ 已支持`
  - 代码已实现。
  - 仓库内有测试覆盖。
  - README / ROADMAP 可以对外说明。
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

`rginx` 的目标已经进一步收口：

- 面向中小规模部署的 Rust 入口反向代理。
- 后续能力投资聚焦 TLS 终止、Host/Path 路由、上游代理、健康检查、热重载、流量治理和可观测性。
- 不再把“本地文件映射 / 静态站点服务”当成主产品方向。
- 不追求在当前阶段成为其他成熟入口代理的 drop-in replacement。

## 当前版本线

当前正式发布线收口为 `v0.1.1`。

当前代码基线已经完成“本地内容服务 -> 纯反向代理”的第一轮收口。后续版本线会继续围绕 proxy path 做协议、调度、健康检查和运维能力补强。

这条版本线接下来的工程重点是：

1. 把产品边界从“入口代理 + 本地内容服务”收口为“入口反向代理”。
2. 消除配置模型、运行时分支、测试和文档里的历史包袱。
3. 把释放出来的复杂度预算转投到真正影响上线可信度的代理能力。

## 新产品边界

目标态 `rginx` 应当满足下面这些约束：

- 保留 `Proxy` 作为主路径能力。
- 保留 `Return`、`Status`、`Metrics`、`Config` 这类运维和控制面 handler。
- 移除 `Static` 和 `File` 这两类依赖本地文件系统或内嵌响应体的内容服务主路径。
- 配置、校验、编译、运行时、测试和示例都不再围绕本地文件服务设计。
- 后续主要投入 upstream 选择、重试、主动健康检查、gRPC / grpc-web、HTTP/2、Upgrade、TLS 和 observability。

下面的“能力矩阵”描述的是当前代码现状。

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
| `Static` handler | ❌ | 已移除，不再支持。 |
| `Proxy` handler | ✅ | 支持上游代理、header 改写、failover 等。 |
| `File` handler | ❌ | 已移除，不再支持。 |
| `Return` handler | ✅ | 支持状态码、Location 和可选响应体。 |
| `Status` handler | ✅ | 返回运行时 JSON 状态。 |
| `Metrics` handler | ✅ | 返回 Prometheus 文本格式指标。 |
| `Config` handler | ✅ | 受限的动态配置管理入口。 |
| 管理 handler 路由约束 | ✅ | `Config` handler 要求 `Exact(...)` 且必须配置非空 `allow_cidrs`。 |

### 4. 本地内容服务

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| 本地文件映射 | ❌ | 已移除，不再作为产品能力。 |
| `try_files` | ❌ | 已移除。 |
| `autoindex` | ❌ | 已移除。 |
| 本地 `Range` 响应 | ❌ | 已移除。 |
| 本地目录列表 | ❌ | 已移除。 |

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
| `/metrics` Prometheus | ✅ | HTTP、gRPC、reload、health check、upstream、failover、peer transition 等指标已覆盖。 |
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
- `rginx-config/src/compile.rs` 已开始继续按 `runtime / server / upstream / route / vhost` 子领域拆分。
- 请求处理主入口已经收口到 `handler/dispatch.rs`，而不是把所有 handler 逻辑混在一个文件里。
- proxy 主逻辑已经按“client / request body / forward / health / grpc-web / upgrade”拆开。
- `proxy/health` 内部已经继续按 `registry / probe / grpc_health_codec` 子领域收口，降低了 health 相关实现的认知负担。
- 集成测试里重复的 child 启动、ready 检查和日志收集已经提取到共享 harness。

### 仍然值得持续关注的大文件

- [`crates/rginx-core/src/config.rs`](crates/rginx-core/src/config.rs)
- [`crates/rginx-config/src/validate.rs`](crates/rginx-config/src/validate.rs)

这些文件当前的问题不是职责混乱，而是体量大、领域细节密度高。下一步如果继续拆，最自然的方向不是“为了拆而拆”，而是按子领域边界拆：

- 配置编译：`runtime / server / upstream / route / vhost`
- 配置校验：`runtime / server / upstream / route / vhost`
- 核心模型：`route / access_log / upstream`

## 纯反向代理化分阶段计划

下面的阶段不是对外版本承诺，而是按当前代码形态最自然、风险最低的收口顺序。

### Phase 1：产品边界冻结

状态：`✅`

目标：

- 先把“什么保留、什么删除”定死，避免后面一边删静态文件能力，一边又继续往里面加功能。
- 让文档和示例先反映新方向，避免继续误导后续实现。

主要动作：

- 在 README、ROADMAP、安装示例和默认配置说明里，把产品定位统一成“反向代理”。
- 明确 `Static` / `File` 进入废弃移除路径，不再作为推荐能力写进对外描述。
- 列出保留的 handler：`Proxy`、`Return`、`Status`、`Metrics`、`Config`。

涉及模块：

- [`README.md`](README.md)
- [`ROADMAP.md`](ROADMAP.md)
- [`configs/rginx.ron`](configs/rginx.ron)
- [`configs/`](configs)

完成标志：

- 仓库内公开文档不再把本地文件服务当成主产品能力。
- 新默认示例配置以 upstream proxy 为中心，而不是静态返回或本地文件映射。

### Phase 2：配置模型收口

状态：`✅`

目标：

- 先从配置层删除无效产品方向，避免 runtime 继续背着历史分支。
- 把“静态内容服务”从 schema、校验和编译链路里整体移除。

主要动作：

- 删除 `HandlerConfig::Static`、`HandlerConfig::File`。
- 删除对应的 route validation 和 route compile 分支。
- 收紧配置错误信息，让新版本明确提示这些 handler 已不再支持。

涉及模块：

- [`crates/rginx-config/src/model.rs`](crates/rginx-config/src/model.rs)
- [`crates/rginx-config/src/validate/route.rs`](crates/rginx-config/src/validate/route.rs)
- [`crates/rginx-config/src/compile/route.rs`](crates/rginx-config/src/compile/route.rs)
- [`crates/rginx-core/src/config.rs`](crates/rginx-core/src/config.rs)

完成标志：

- 配置文件里无法再声明 `Static` / `File`。
- 编译后的运行时快照不再携带本地文件服务相关模型。

### Phase 3：HTTP 运行时裁剪

状态：`✅`

目标：

- 删除运行时里不再需要的分支和文件系统耦合，把请求主路径收紧到真正的代理和管理逻辑。

主要动作：

- 从 dispatch 分发逻辑里移除 `Static` / `File` 分支。
- 删除本地文件处理模块及其对 MIME、Range、目录列表等逻辑的依赖。
- 清理 `lib.rs` 与模块导出，确保 `rginx-http` 的职责围绕 server / handler / proxy。

涉及模块：

- [`crates/rginx-http/src/handler/dispatch.rs`](crates/rginx-http/src/handler/dispatch.rs)
- [`crates/rginx-http/src/lib.rs`](crates/rginx-http/src/lib.rs)

完成标志：

- HTTP 请求处理主路径里不再存在本地文件或静态 body handler 分支。
- `rginx-http` 不再包含为文件服务存在的大块专用代码。

### Phase 4：测试与示例回收

状态：`✅`

目标：

- 删除已经不符合产品边界的测试资产，避免未来维护成本继续花在非目标能力上。
- 把测试预算转到 proxy、failover、health、reload 和协议兼容路径。

主要动作：

- 删除或改写覆盖 `Static` / `File` 的集成测试。
- 更新默认配置和示例配置，使其体现反向代理、管理接口、健康检查和负载均衡。
- 清理 README 中关于 `root`、`index`、`try_files`、`Range` 等说明。

涉及模块：

- [`crates/rginx-app/tests/phase1.rs`](crates/rginx-app/tests/phase1.rs)
- [`crates/rginx-app/tests/compression.rs`](crates/rginx-app/tests/compression.rs)
- [`crates/rginx-app/tests/`](crates/rginx-app/tests)
- [`README.md`](README.md)
- [`configs/`](configs)

完成标志：

- 工作区测试不再依赖本地文件服务能力。
- README、示例配置、集成测试三者对外口径一致。

### Phase 5：代理能力补强

状态：`✅`

目标：

- 把前面收回来的复杂度预算投入真正有价值的代理能力。
- 让 `rginx` 在“做反向代理”这件事上明显更强，而不是功能更杂。

主要动作：

- 加强 upstream retry / failover / health / load balancing 语义和观测。
- 新增 failover 计数与 peer health transition 指标，让主动/被动健康状态变化可直接从 Prometheus 观察。
- 收紧被动健康状态机语义，避免重复上报“已不健康 peer 再次失败”的转坏事件。

涉及模块：

- [`crates/rginx-http/src/proxy/forward.rs`](crates/rginx-http/src/proxy/forward.rs)
- [`crates/rginx-http/src/proxy/clients.rs`](crates/rginx-http/src/proxy/clients.rs)
- [`crates/rginx-http/src/proxy/health.rs`](crates/rginx-http/src/proxy/health.rs)
- [`crates/rginx-http/src/proxy/health/registry.rs`](crates/rginx-http/src/proxy/health/registry.rs)
- [`crates/rginx-http/src/compression.rs`](crates/rginx-http/src/compression.rs)
- [`crates/rginx-app/tests/grpc_proxy.rs`](crates/rginx-app/tests/grpc_proxy.rs)
- [`crates/rginx-app/tests/http2.rs`](crates/rginx-app/tests/http2.rs)
- [`crates/rginx-app/tests/upstream_http2.rs`](crates/rginx-app/tests/upstream_http2.rs)
- [`crates/rginx-app/tests/failover.rs`](crates/rginx-app/tests/failover.rs)
- [`crates/rginx-app/tests/backup.rs`](crates/rginx-app/tests/backup.rs)
- [`crates/rginx-app/tests/ip_hash.rs`](crates/rginx-app/tests/ip_hash.rs)
- [`crates/rginx-app/tests/least_conn.rs`](crates/rginx-app/tests/least_conn.rs)
- [`crates/rginx-app/tests/active_health.rs`](crates/rginx-app/tests/active_health.rs)
- [`crates/rginx-app/tests/reload.rs`](crates/rginx-app/tests/reload.rs)
- [`crates/rginx-app/tests/dynamic_config_api.rs`](crates/rginx-app/tests/dynamic_config_api.rs)

完成标志：

- 主要测试资产集中在代理路径而不是文件服务。
- 新增复杂度优先落在协议、调度、健康检查和可观测性，而不是内容分发分支。
- `/metrics` 可区分 upstream failover、主动健康转坏/恢复、被动健康转坏/恢复。

### Phase 6：模块进一步按代理领域细化

状态：`✅`

目标：

- 在产品边界已经收紧后，再继续拆大文件，避免一边删能力一边重构两次。

主要动作：

- 按 proxy domain 继续整理核心配置和校验模块。
- 让后续增量需求更容易落在明确的子领域文件，而不是再次回到巨型入口文件。

涉及模块：

- [`crates/rginx-core/src/config.rs`](crates/rginx-core/src/config.rs)
- [`crates/rginx-config/src/validate.rs`](crates/rginx-config/src/validate.rs)

完成标志：

- 大文件体量下降，但职责边界仍然清晰。
- 后续 proxy feature 的改动落点比现在更稳定。

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

如果三者不同步，后续维护成本会上升得非常快。

## 参考

- 顶层说明：[README.md](README.md)
- 默认配置示例：[configs/rginx.ron](configs/rginx.ron)
