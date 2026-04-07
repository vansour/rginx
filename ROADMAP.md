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

这里的“反向代理”需要理解成专项替代，而不是全量替代：

- 当前替代目标是“nginx 用作入口反向代理 / API gateway / gRPC ingress”的常见子集。
- 当前不追求替代 nginx 在静态文件、通用配置 DSL、L4 `stream`、mail、FastCGI 等方向上的历史能力。

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
- 保留 `Return` 这类运维 handler。
- 移除 `Static`、`File`、`Status`、`Metrics`、`Config` 这类内容服务和控制面 handler。
- 配置、校验、编译、运行时、测试和示例都不再围绕管理接口设计。
- 后续主要投入 upstream 选择、重试、主动健康检查、gRPC / grpc-web、HTTP/2、Upgrade、TLS 和 observability。

下面的“能力矩阵”描述的是当前代码现状。

## 专项反向代理替代合同

这不是营销文案，而是决定“什么可以写进对外承诺、什么不该插队进入主线”的实际合同。

### 目标场景

当前版本线应优先覆盖下面这些场景：

- 中小规模部署下的 HTTP / HTTPS 入口反向代理
- API gateway 前置代理
- gRPC ingress 和 grpc-web 入口转换
- 边缘节点反代，或部署在 LB / CDN 后方的入口代理
- TLS 终止、Host/Path 路由、upstream 负载均衡、健康检查、基础流量治理、热重载和可观测性

### 明确非目标

下面这些方向当前不应被当作主线投入：

- 本地静态文件、本地内容分发和网站托管
- 远程 HTTP 管理面、公网 admin 路由、动态配置 API
- 通用入口代理的全量 drop-in replacement
- 完整 nginx 配置语法兼容
- `stream` / `mail` / FastCGI / uwsgi / scgi 这类非当前主线协议能力

### 写进稳定承诺的最低条件

某项能力只有同时满足下面这些条件，才应写进稳定承诺：

- 代码已实现
- 仓库内已有测试覆盖
- README / ROADMAP 已明确说明边界
- 默认配置、示例配置或 CLI 中已有合理使用落点
- 失败语义和不支持边界已经写清

如果一个需求不能稳定落在目标场景里，或者暂时无法满足上面的承诺条件，它就不应挤占当前版本线的主路径预算。

## 能力矩阵

### 1. 配置与运行时

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| RON 顶层配置 | ✅ | `Config(runtime, server, upstreams, locations, servers)` 已稳定。 |
| 配置加载 | ✅ | 支持从显式路径或默认路径加载。 |
| 相对 `include` | ✅ | 支持 `// @include "relative/path.ron"` 文本级拼接。 |
| 字符串环境变量展开 | ✅ | 支持 `${VAR}` 与 `${VAR:-default}`。 |
| `rginx check` | ✅ | 会走完整加载、校验和编译链路。 |
| 配置语义校验 | ✅ | 字段合法性、跨字段约束、gRPC 路由约束都已前置。 |
| 编译为运行时快照 | ✅ | 启动与重载都统一编译为 `ConfigSnapshot`。 |
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
| `Status` handler | ❌ | 已移除，转为本地运维命令。 |
| `Metrics` handler | ❌ | 已移除，转为日志输出。 |
| `Config` handler | ❌ | 已移除，改为文件配置 + reload 模式。 |

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
| `Ctrl-C` / `SIGTERM` 平滑退出 | ✅ | 支持。 |
| `SIGHUP` 热重载 | ✅ | 支持。 |
| 安装 / 卸载脚本 | ✅ | 仓库内已提供。 |
| Release workflow | ✅ | 已有准备脚本和 release 文档。 |
| `rginx status` 本地命令 | ❌ | 未实现。 |
| `/status` JSON | ❌ | 已移除。 |
| `/metrics` Prometheus | ❌ | 已移除，指标仅通过 access log 观察。 |
| 内建 systemd / service unit | ❌ | 当前不内置。 |
| OpenTelemetry | ❌ | 当前不支持。 |

### 9. 工程化与测试

| 能力项 | 状态 | 说明 / 边界 |
| :--- | :--- | :--- |
| crate 分层 | ✅ | `app / config / core / http / runtime / observability` 职责明确。 |
| 配置加载 / 校验 / 编译分层 | ✅ | 已拆清。 |
| `handler/` 目录化 | ✅ | 已从单大文件拆成 `dispatch / response / grpc / access_log`。 |
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
- 不提供远程动态配置 API；配置变更通过“写文件 + `check` + reload”完成。
- 热重载不能切换 `listen`、`runtime.worker_threads` 或 `runtime.accept_workers`。
- 单实例当前围绕一个监听地址工作，不是多 `listen` 入口模型。

### 代理与兼容性边界

- 只承诺基础 grpc-web binary / text 模式，不承诺更完整高级 grpc-web 兼容。
- 不支持 Proxy Protocol。
- 当前产品目标不是其他成熟入口代理的全量语义兼容实现。

### 运维与发布边界

- 默认假设前面如果存在 LB / CDN / 其他代理，部署者会正确配置 `trusted_proxies`。
- 默认假设运维观察通过 access log、本机日志和外部 supervisor 完成，而不是通过 HTTP 管理路由。
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
- 列出保留的 handler：`Proxy`、`Return`。

---

### Phase 1.1：HTTP 管理面冻结

状态：`🚧`

目标：

- 明确 `Status` / `Metrics` / `Config` 进入废弃移除路径。
- 运维方式收口为：修改配置文件、执行 `check`、发送 reload 信号、查看本地日志。

详见本节上下文与当前能力矩阵。

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
- 加强 failover 与 peer health transition 的日志和测试覆盖，让主动/被动健康状态变化可被稳定观察。
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

完成标志：

- 主要测试资产集中在代理路径而不是文件服务。
- 新增复杂度优先落在协议、调度、健康检查和可观测性，而不是内容分发分支。
- upstream failover、主动健康转坏/恢复、被动健康转坏/恢复都已有测试覆盖与日志信号。

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

## 8 周执行路线图

这份 8 周路线图不是“8 周后一定全部完成”的对外承诺，而是基于当前代码基线、当前产品边界和“专项反向代理替代 nginx”这个目标，给出的最稳妥推进顺序。

执行假设：

- 继续坚持“纯反向代理数据面”边界，不回头恢复公网 HTTP 管理面、本地文件服务和动态配置 API。
- 目标是替代“nginx 用作入口反向代理 / API gateway / gRPC ingress”的常见子集，不追求 full drop-in replacement。
- 每周默认包含代码、测试、README / ROADMAP 同步，而不是只交代码。
- 如果中途资源不足，优先保 Week 1 到 Week 6，Week 7 到 Week 8 可以顺延，但不应反向侵蚀前面的基础闭环。

阶段护栏：

- Week 1 到 Week 3 的目标是补齐“本地可运维性”。
- Week 4 到 Week 6 的目标是补齐“入口模型和进程生命周期”。
- Week 7 到 Week 8 的目标是补齐“部署兼容性、迁移体验和上线可信度”。

### Week 1：替代边界冻结与验收合同

目标：

- 把“替代什么、不替代什么、做到什么算可上线”写成正式合同，避免后续 feature 漂移。

核心动作：

- 在 README 和 ROADMAP 里补一份明确的 capability contract。
- 把目标场景写死为：API 反向代理、gRPC ingress、边缘反代、TLS 终止、Host/Path 路由、基础流量治理、健康检查、热重载、可观测性。
- 把非目标写死为：静态文件、本地内容服务、远程 admin API、完整 nginx 语法兼容、L4 stream、mail、FastCGI。
- 为后续 7 周拆出明确的 issue / milestone 列表，按“必须完成 / 可以顺延”分级。

交付物：

- 更新后的 README 能力边界说明。
- 更新后的 ROADMAP 能力矩阵与非目标说明。
- 一个按周拆分的 backlog 列表。

验收标准：

- 后续新增需求都能判断是否属于“专项反向代理替代品”范围。
- 对外不再出现“入口代理 + 其他能力”的模糊表述。

### Week 2 到 Week 8 backlog 交付约定

从 Week 2 开始，执行项不应只存在于路线图正文里，而应同时具备下面两层载体：

- 文档层：保留在本文件里的按周计划、目标和验收标准
- 执行层：拆成真实的 milestone 和 issues，并按“必须完成 / 可以顺延”分级

对 Week 2 到 Week 8 的 issue 拆分，最低要求是：

- 每周至少有一个对应的主 issue
- 每个主 issue 都显式写出“必须完成”
- 每个主 issue 都显式写出“可以顺延”
- 每个主 issue 都显式写出验收标准

只要执行层 backlog 还没建起来，Week 1 就不算真正完成。

当前执行层 backlog 已同步到 GitHub：

- milestone: [专项反向代理替代路线图（8 周主线）](https://github.com/vansour/rginx/milestone/1)
- Week 2: [#13](https://github.com/vansour/rginx/issues/13)
- Week 3: [#14](https://github.com/vansour/rginx/issues/14)
- Week 4: [#15](https://github.com/vansour/rginx/issues/15)
- Week 5: [#16](https://github.com/vansour/rginx/issues/16)
- Week 6: [#17](https://github.com/vansour/rginx/issues/17)
- Week 7: [#18](https://github.com/vansour/rginx/issues/18)
- Week 8: [#19](https://github.com/vansour/rginx/issues/19)

### Week 2：本地只读运维面设计与状态快照内核

目标：

- 先补齐内部可读取状态，把“删掉公网 admin”真正闭环成“本地可运维”。

核心动作：

- 设计本地只读运维接口模型，优先走 UDS，与 [`EDGE_CONTROL_PLANE_PLAN.md`](EDGE_CONTROL_PLANE_PLAN.md) 保持一致。
- 在 `SharedState` 上补充可快照的运行时状态：当前 revision、active connections、最近 reload 结果、基础 counters。
- 在 `PeerHealthRegistry` 和 `ProxyClients` 上补充只读 snapshot 能力。
- 明确第一版状态集：`GetStatus`、`GetCounters`、`GetPeerHealth`、`GetRevision`。

交付物：

- UDS 管理面 RFC 或实现草案。
- 运行时状态快照结构体。
- 单元测试，覆盖状态读取的一致性和并发安全性。

验收标准：

- 不依赖公网端口，也能从进程内稳定读取当前运行状态。
- 状态模型足够支撑后续 CLI 和 `edge-agent` 集成。

### Week 3：本地只读运维面落地

目标：

- 交付真正可用的本地运维入口，而不是只停留在内部状态结构。

核心动作：

- 在 runtime 内增加 UDS 只读服务，例如 `/run/rginx/admin.sock`。
- 在 CLI 中增加本地查询命令，例如 `rginx status`、`rginx peers`、`rginx counters`。
- 为本地 UDS 和 CLI 补集成测试。
- 更新文档，明确运维方式已经收口为“check / reload / 本地状态读取 / 日志”。

交付物：

- 本地只读 UDS 管理面实现。
- 新 CLI 子命令。
- 集成测试与示例输出。

验收标准：

- 不暴露公网管理路由。
- 运维人员可以在本机查询 revision、连接数、upstream peer 健康状态和基础 counters。

### Week 4：多监听入口模型设计与配置兼容方案

目标：

- 把当前“单实例单 listen”限制升级为“可覆盖典型 nginx 反代入口模型”的配置设计。

核心动作：

- 设计顶层 `listeners: []` 或等价模型，让监听配置和虚拟主机配置解耦。
- 明确 listener 负责的字段：监听地址、TLS、连接限制、读写超时、trusted proxies。
- 明确 vhost 负责的字段：`server_names`、routes、可选证书覆盖。
- 设计旧配置到新配置的兼容编译路径，避免一次性打断当前用户。

交付物：

- 新配置模型设计。
- 向后兼容策略。
- 配置校验和编译的迁移方案。

验收标准：

- 能清楚表达 `:80`、`:443`、IPv4 / IPv6、多入口部署需求。
- 旧的 `server.listen` 模型仍能被编译为默认 listener。

### Week 5：多监听入口模型实现

目标：

- 真正支持常见 nginx 反代部署形态，而不只是完成配置设计。

核心动作：

- 修改 `rginx-config` 的 `model / validate / compile` 链路，实现多 listener。
- 修改 runtime bootstrap 和 HTTP server accept loop，使单进程可持有多个 listener 组。
- 补端到端测试：`:80 + :443`、双 listener、默认 redirect host、IPv4 / IPv6 组合。
- 保持现有 TLS/SNI 逻辑在多 listener 下仍然可解释。

交付物：

- 多 listener 的配置编译和运行时实现。
- 向后兼容测试。
- 典型部署示例配置。

验收标准：

- 单进程同时监听多个入口地址。
- 能支持最常见的 `80 -> 443` 和多域名 TLS 入口部署。

### Week 6：优雅重启路径打通

目标：

- 解决当前 `SIGHUP` 不能切 `listen` / `worker_threads` / `accept_workers` 的现实短板。

核心动作：

- 在 Linux 上选择一条主路径实现优雅重启：优先二选一，而不是同时铺开。
- 备选方案 A：systemd socket activation。
- 备选方案 B：显式 fd 继承 + exec restart。
- 保持现有配置热重载逻辑，用“热重载处理可原地替换项，优雅重启处理监听和 runtime 结构变化”分层解决。
- 补 listener 交接、连接 drain、失败回退测试。

交付物：

- Linux 主路径优雅重启实现。
- 对应 CLI / 运维说明。
- 进程切换和 drain 测试。

验收标准：

- 修改监听地址或 runtime worker 参数时，不再只能硬停进程。
- 新旧进程交接期间不出现明显的连接丢失和状态失真。

### Week 7：部署兼容性补强

目标：

- 补齐真实部署里最常见、最容易卡住替代落地的兼容能力。

核心动作：

- 支持 upstream hostname 的 DNS 重解析与刷新。
- 支持 inbound `PROXY protocol`，与 `trusted_proxies`、`X-Forwarded-For` 语义打通。
- 为后置于 LB / CDN / 四层代理后的部署补示例与回归测试。
- 明确这些能力的失败语义、刷新周期和默认安全边界。

交付物：

- DNS 重解析实现。
- inbound `PROXY protocol` 实现。
- 相关测试和部署文档。

验收标准：

- upstream 指向域名时，后端 IP 变化不需要靠手动重启追踪。
- LB / CDN 后置部署中，客户端真实地址链路可稳定保留。

### Week 8：迁移体验、性能基线与发布收口

目标：

- 让项目从“有能力”进入“可迁移、可验证、可发布”的状态。

核心动作：

- 补一份“nginx 反向代理子集 -> rginx 配置模型”的迁移手册。
- 视时间实现一个最小迁移辅助工具，至少覆盖 `listen / server_name / location / proxy_pass / proxy_set_header / client_max_body_size / upstream weight / backup`。
- 固定一套 benchmark 和 soak test 场景：HTTP/1.1、TLS termination、HTTP/2、gRPC、grpc-web、Upgrade、reload / restart 期间连接稳定性。
- 更新 release checklist、systemd / supervisor 建议和上线说明。

交付物：

- nginx 子集迁移文档。
- benchmark / soak test 报告。
- 发布与部署收口文档。

验收标准：

- 常见 nginx API 反代配置已有清晰迁移路径。
- 发布文档开始包含性能和容量边界，而不只是功能列表。

### 8 周后应达到的最低结果

如果这 8 周按上面的优先级完成，`rginx` 至少应达到下面这个替代门槛：

- 不再依赖公网 admin 路由，也具备本地可运维性。
- 可以覆盖最常见的多入口 nginx 反向代理部署形态。
- 监听和 runtime 结构变化不再只能靠硬停进程处理。
- 在 LB / CDN / 域名 upstream 这类真实部署里不再缺关键兼容件。
- 有一套可执行的迁移文档、性能基线和发布收口方法。

### 8 周内不建议插队的方向

为了保证路线图收敛，下面这些方向不建议在 8 周窗口内抢占主线：

- 恢复公网 HTTP admin surface。
- 恢复静态文件或本地内容服务。
- 追求完整 nginx 配置语法兼容。
- 优先做 HTTP/3，而不是先补多 listener、优雅重启和本地运维面。
- 没有真实场景驱动的额外负载均衡策略或语法糖。

## 文档与测试同步约定

以后新增或调整能力时，建议把下面几项当成一套动作，而不是只改代码：

1. 补或更新单元测试 / 集成测试。
2. 更新 README 的能力描述或示例。
3. 更新本文件里的能力矩阵、限制或阶段规划。

如果三者不同步，后续维护成本会上升得非常快。

## 参考

- 顶层说明：[README.md](README.md)
- 默认配置示例：[configs/rginx.ron](configs/rginx.ron)
