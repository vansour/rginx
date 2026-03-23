# Architecture

本页从代码结构和运行时两个维度说明 `Rginx` 的整体架构。

## 工作区结构

`Rginx` 是一个 Cargo workspace，核心 crate 分工如下：

| crate | 作用 |
| --- | --- |
| `crates/rginx-app` | 二进制入口、CLI 解析、启动与 `check` 命令 |
| `crates/rginx-config` | 配置加载、语义校验、编译为运行时快照 |
| `crates/rginx-core` | 共享模型、错误类型、配置快照结构 |
| `crates/rginx-http` | HTTP 服务器、路由、代理、文件服务、指标与状态页 |
| `crates/rginx-runtime` | 运行时编排、热重载、信号处理、主动健康检查任务 |
| `crates/rginx-observability` | 日志初始化与 tracing 集成 |

## 启动链路

入口在 `crates/rginx-app/src/main.rs`。

启动过程可以概括为：

1. 解析 CLI 参数。
2. 初始化日志。
3. 读取并编译配置。
4. 如果是 `check`，只做校验并输出摘要。
5. 如果是正常运行，进入 `rginx_runtime::run(...)`。

## 运行时生命周期

运行时主循环在 `crates/rginx-runtime/src/bootstrap.rs`。

关键阶段：

1. 构建 `RuntimeState`，其中包含配置文件路径和 `SharedState`。
2. 绑定监听地址。
3. 启动 HTTP 服务任务。
4. 启动主动健康检查后台任务。
5. 等待信号：
   - `SIGHUP` 触发热重载
   - `Ctrl-C` / `SIGTERM` 触发平滑退出
6. 平滑退出时，等待连接和后台任务在 `shutdown_timeout_secs` 内收敛。

## 配置到运行时的转换

配置链路分三层：

1. `load.rs`
   - 读取文件
   - 解析 RON
2. `validate.rs`
   - 校验字段合法性
   - 校验跨字段约束
3. `compile.rs`
   - 归一化路径和默认值
   - 构建运行时 `ConfigSnapshot`
   - 把 handler、upstream、vhost 等转换为运行时结构

这样做的好处是：

- 启动阶段尽量把错误前置
- 请求路径里只处理编译后的稳定结构
- 热重载复用同一条编译链路

## 请求处理链路

请求主路径在 `crates/rginx-http/src/server.rs` 和 `crates/rginx-http/src/handler.rs`。

```mermaid
flowchart TD
    A[TcpListener accept] --> B{TLS enabled?}
    B -- no --> C[HTTP/1.1 connection]
    B -- yes --> D[TLS handshake]
    D --> E{ALPN == h2?}
    E -- yes --> F[HTTP/2 connection]
    E -- no --> C
    C --> G[handler::handle]
    F --> G
    G --> H[resolve request id]
    H --> I[resolve client ip]
    I --> J[select vhost]
    J --> K[select route]
    K --> L{route action}
    L -- Static --> M[static response]
    L -- Proxy --> N[proxy::forward_request]
    L -- File --> O[file::serve_file]
    L -- Return --> P[return response]
    L -- Status --> Q[/status JSON]
    L -- Metrics --> R[/metrics text]
    M --> S[access log + metrics]
    N --> S
    O --> S
    P --> S
    Q --> S
    R --> S
```

## HTTP 层的关键模块

| 模块 | 作用 |
| --- | --- |
| `server.rs` | accept loop、TLS/ALPN 分流、连接级平滑退出 |
| `handler.rs` | request id、真实客户端 IP、路由执行、状态页、指标页 |
| `router.rs` | vhost 选择与路径匹配 |
| `proxy.rs` | upstream 选择、转发、重试、健康状态、upgrade 隧道 |
| `file.rs` | 静态文件、`HEAD`、`Range`、`try_files` |
| `client_ip.rs` | `trusted_proxies` 与 `X-Forwarded-For` 解析 |
| `rate_limit.rs` | 路由级 token bucket |
| `metrics.rs` | Prometheus 指标聚合与输出 |
| `state.rs` | 共享状态、配置替换、后台任务管理 |

## 状态模型

运行时有两个核心状态对象：

- `RuntimeState`
  - 位于 `crates/rginx-runtime/src/state.rs`
  - 保存配置文件路径与 `SharedState`
- `SharedState`
  - 位于 `crates/rginx-http/src/state.rs`
  - 保存当前配置快照、proxy clients、TLS acceptor、指标、限流器、后台任务和配置修订号

`SharedState::replace(...)` 用于热重载时替换整套运行时配置。

## 连接与后台任务

除了正常的 HTTP 连接，`Rginx` 还有两类后台任务：

- 主动健康检查任务
- Upgrade/WebSocket 隧道转发任务

平滑退出时会优先等待它们自然结束，超时后再执行 abort。

## 为什么是快照式配置

`Rginx` 没有在请求路径里逐项查表解析原始配置，而是先把配置编译成 `ConfigSnapshot`。这样可以：

- 避免请求期间反复做字符串解析
- 让 reload 成为“整套替换”，而不是“局部修改”
- 简化 handler / proxy 层的逻辑分支

## 继续阅读

- [Configuration](Configuration.md)
- [Routing and Handlers](Routing-and-Handlers.md)
- [Upstreams](Upstreams.md)
- [Operations](Operations.md)
