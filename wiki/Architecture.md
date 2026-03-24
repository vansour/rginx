# Architecture

本页从代码结构、启动链路、请求链路和运行时状态四个维度说明 `rginx` 的整体架构。

如果你准备动代码，建议把本页和 [Development](Development.md) 一起看。

## 总体设计

`rginx` 当前采用的是比较明确的“先编译配置，再跑请求”的结构：

1. 把原始 RON 配置加载、展开、校验、编译成稳定的运行时快照。
2. 用这份快照构建 HTTP 服务所需的共享状态。
3. 请求路径里尽量只操作编译后的结构，而不是反复解析原始配置。
4. 热重载时直接整套替换当前快照和相关派生对象。

这带来几个直接好处：

- 启动期尽量把配置错误前置。
- 请求路径的逻辑更接近“查表 + 执行动作”。
- reload 更像“切换快照”，而不是在运行时做局部打补丁。

## 工作区结构

`rginx` 是一个 Cargo workspace，核心 crate 如下：

| crate | 作用 |
| --- | --- |
| `crates/rginx-app` | 二进制入口、CLI、`check` 命令、集成测试 |
| `crates/rginx-config` | 配置加载、`include` / 环境变量展开、校验、编译 |
| `crates/rginx-core` | 共享运行时模型、错误类型、upstream 选择逻辑 |
| `crates/rginx-http` | HTTP 服务器、handler、proxy、文件服务、限流、指标、TLS |
| `crates/rginx-runtime` | 运行时编排、热重载、信号处理、主动健康检查 |
| `crates/rginx-observability` | tracing / logging 初始化 |

当前最值得记住的目录边界是：

```text
crates/rginx-app/src/
  main.rs
  cli.rs

crates/rginx-config/src/
  load.rs
  validate.rs
  compile.rs
  model.rs

crates/rginx-http/src/
  server.rs
  state.rs
  router.rs
  file.rs
  metrics.rs
  rate_limit.rs
  client_ip.rs
  compression.rs
  timeout.rs
  tls.rs
  handler/
    mod.rs
    dispatch.rs
    admin.rs
    grpc.rs
    access_log.rs
  proxy/
    mod.rs
    clients.rs
    request_body.rs
    forward.rs
    health.rs
    grpc_web.rs
    upgrade.rs
```

## 启动链路

入口在 [`crates/rginx-app/src/main.rs`](../crates/rginx-app/src/main.rs)。

启动流程可以概括为：

1. 解析 CLI。
2. 初始化 logging / tracing。
3. 通过 `rginx_config::load_and_compile(...)` 读取并编译配置。
4. 如果是 `check` 子命令，只验证配置是否可用，并构造一次 `SharedState` 做运行时依赖校验。
5. 如果是正常启动，构建 tokio runtime，并进入 `rginx_runtime::run(...)`。

对应入口文件：

- `crates/rginx-app/src/main.rs`
- `crates/rginx-app/src/cli.rs`
- `crates/rginx-observability/src/logging.rs`

## 配置链路

配置链路当前分三层：

### 1. `load.rs`

负责：

- 读取主配置文件
- 解析 `// @include "..."` 指令
- 展开双引号字符串内的 `${VAR}` / `${VAR:-default}`
- 最终把展开后的文本交给 RON 解析

### 2. `validate.rs`

负责：

- 字段合法性校验
- 跨字段语义校验
- 管理路由约束，例如 `Config` handler 必须是 `Exact(...)` 且必须有 `allow_cidrs`
- gRPC route 约束
- server / vhost / upstream / route 的重复项和非法组合校验

### 3. `compile.rs`

负责把原始 `model.rs` 里的配置编译成运行时结构：

- 解析 socket address、CIDR、TLS 模式、path
- 计算默认值
- 生成 `ConfigSnapshot`
- 把 route / upstream / vhost 转成请求路径里直接使用的结构

配置编译完成后的核心运行时模型位于：

- [`crates/rginx-core/src/config.rs`](../crates/rginx-core/src/config.rs)

## 运行时生命周期

运行时主循环在 [`crates/rginx-runtime/src/bootstrap.rs`](../crates/rginx-runtime/src/bootstrap.rs)。

关键阶段：

1. 基于 `config_path + ConfigSnapshot` 构建 `RuntimeState`。
2. 绑定监听地址，并按 `accept_workers` clone listener。
3. 为每个 accept worker 启动一个 HTTP 服务任务。
4. 启动主动健康检查后台任务。
5. 等待信号：
   - `SIGHUP` -> reload
   - `Ctrl-C` / `SIGTERM` -> graceful shutdown
6. 退出时等待连接与后台任务在 `shutdown_timeout_secs` 内收敛；超时后再 abort。

## 共享状态模型

运行时里有两个关键状态对象：

### `RuntimeState`

位置：

- [`crates/rginx-runtime/src/state.rs`](../crates/rginx-runtime/src/state.rs)

作用：

- 保存配置文件路径
- 保存 HTTP 层共享状态 `SharedState`

### `SharedState`

位置：

- [`crates/rginx-http/src/state.rs`](../crates/rginx-http/src/state.rs)

作用：

- 保存当前 revision
- 保存当前 `ConfigSnapshot`
- 保存编译派生出的 `ProxyClients`
- 保存当前 TLS acceptor
- 保存 config source 文本
- 保存 metrics、rate limiters、background tasks、request id 计数器

`SharedState` 是热重载和管理接口的中心。

当 reload 或动态配置更新发生时，核心动作不是“修改当前对象里的几个字段”，而是：

1. 编译出一套新的 `ConfigSnapshot`
2. 预先构建派生依赖，例如 `ProxyClients` 和 TLS acceptor
3. 验证是不是允许热替换
4. 原子替换当前 active state

## 请求处理链路

请求主路径涉及三个层次：

1. `server.rs` 负责 accept、TLS、ALPN、连接级 graceful shutdown
2. `handler/dispatch.rs` 负责请求级调度
3. `proxy/`、`file.rs`、`admin.rs` 等负责具体行为

### 连接级流程

入口在 [`crates/rginx-http/src/server.rs`](../crates/rginx-http/src/server.rs)。

大致流程：

1. `TcpListener.accept()`
2. 如果配置了 TLS，则做 handshake
3. 如果 ALPN 协商到 `h2`，走 HTTP/2 connection
4. 否则走 HTTP/1.1 connection
5. 每条连接都绑定到同一个 `handler::handle(...)`

连接层还负责：

- `max_connections`
- HTTP/1.1 keep-alive
- header read timeout
- response write timeout
- graceful shutdown

### 请求级流程

主入口在 [`crates/rginx-http/src/handler/dispatch.rs`](../crates/rginx-http/src/handler/dispatch.rs)。

可以抽象成下面这条链：

```mermaid
flowchart TD
    A[Request] --> B[resolve request id]
    B --> C[resolve client ip]
    C --> D[select vhost]
    D --> E[select route]
    E --> F{ACL / rate limit}
    F -- reject --> G[error response]
    F -- pass --> H{route action}
    H -- Static --> I[text response]
    H -- Proxy --> J[proxy::forward_request]
    H -- File --> K[file::serve_file]
    H -- Return --> L[return response]
    H -- Status --> M[/status]
    H -- Metrics --> N[/metrics]
    H -- Config --> O[/-/config]
    I --> P[compression]
    J --> P
    K --> P
    L --> P
    M --> P
    N --> P
    O --> P
    P --> Q[access log + metrics]
```

### 请求路径里发生的关键事情

#### 1. Request ID

- 优先复用下游已有的 `X-Request-ID`
- 没有就由 `SharedState` 生成
- 会写回响应头

#### 2. 客户端 IP 识别

入口：

- [`crates/rginx-http/src/client_ip.rs`](../crates/rginx-http/src/client_ip.rs)

规则：

- 只有对端地址属于 `trusted_proxies` 时，才信任 `X-Forwarded-For`
- 否则直接用 socket peer

#### 3. Host / Path / gRPC 路由选择

入口：

- [`crates/rginx-http/src/router.rs`](../crates/rginx-http/src/router.rs)

规则：

- 先选 vhost
- 再选 route
- 如果请求被识别为 gRPC / grpc-web，会把 `grpc_service` / `grpc_method` 一起纳入 route match context

#### 4. ACL 与限流

入口：

- [`crates/rginx-http/src/handler/dispatch.rs`](../crates/rginx-http/src/handler/dispatch.rs)
- [`crates/rginx-http/src/rate_limit.rs`](../crates/rginx-http/src/rate_limit.rs)

顺序：

- 先做 CIDR allow / deny
- 再做 token bucket 限流

#### 5. route action 执行

分发到：

- `Static` -> `text_response`
- `Proxy` -> `proxy::forward_request`
- `File` -> `file::serve_file`
- `Return` -> 本地返回
- `Status` / `Metrics` / `Config` -> `handler/admin.rs`

#### 6. 响应压缩

入口：

- [`crates/rginx-http/src/compression.rs`](../crates/rginx-http/src/compression.rs)

当前策略：

- 只对适合缓冲的小体积文本响应做基础 br / gzip 协商
- 会跳过 `Range`、已编码响应、gRPC 等不适合场景

#### 7. access log 与 metrics

入口：

- [`crates/rginx-http/src/handler/access_log.rs`](../crates/rginx-http/src/handler/access_log.rs)
- [`crates/rginx-http/src/metrics.rs`](../crates/rginx-http/src/metrics.rs)

## Proxy 子系统

proxy 子系统已经按职责拆开，当前边界比较自然：

| 文件 | 作用 |
| --- | --- |
| `proxy/mod.rs` | 公共 helper、header 清洗、URI 构造、协议判断 |
| `proxy/clients.rs` | 复用的 upstream client 缓存与 TLS profile 选择 |
| `proxy/request_body.rs` | 下游请求体预处理、重放能力判断、grpc-web 请求体转换 |
| `proxy/forward.rs` | 主转发流程、failover、timeout、downstream response 构造 |
| `proxy/health.rs` | passive / active 健康状态、least_conn、主动 probe |
| `proxy/grpc_web.rs` | grpc-web binary / text 请求与响应转换 |
| `proxy/upgrade.rs` | HTTP Upgrade / WebSocket 双向隧道 |

### proxy 主流程

`forward_request(...)` 里主要做了这些事：

1. 判断是否是 grpc-web
2. 计算生效的 upstream request timeout
3. 准备可重放或流式的 request body
4. 根据健康状态和负载均衡策略选 peer
5. 发起 upstream 请求
6. 如果当前请求满足条件，则在 peer 失败后做 failover
7. 把 upstream response 包装成 downstream response
8. 在需要时包上 idle timeout、gRPC deadline、grpc-web 编解码和 active request guard

### 健康检查

proxy health 同时承担两类逻辑：

- 被动健康检查
  - 请求失败会增加失败计数
  - 达到阈值后进入冷却期
- 主动健康检查
  - runtime 后台任务定期发起 probe
  - probe 结果会驱动 active health 状态

least_conn 也依赖同一套 peer health registry 中的 active request 计数。

## TLS 与证书选择

入口：

- [`crates/rginx-http/src/tls.rs`](../crates/rginx-http/src/tls.rs)

当前实现：

- 把 default vhost 和额外 vhost 的 TLS 配置统一收集起来
- 构建一个基于 server name 的证书解析器
- handshake 时根据 SNI 选择证书
- ALPN 默认带上 `h2` 和 `http/1.1`

## 管理接口

管理接口逻辑位于：

- [`crates/rginx-http/src/handler/admin.rs`](../crates/rginx-http/src/handler/admin.rs)

当前主要提供：

- `/status`
- `/metrics`
- `Config` handler 对应的动态配置读取 / 更新

动态配置更新的核心流程是：

1. 读取并校验 HTTP body
2. 用 `load_and_compile_from_str(...)` 编译新配置
3. 检查是否允许热替换
4. 原子写回配置文件
5. 提交新的 active state

## 后台任务

除了正常的 HTTP 连接，当前还有两类重要后台任务：

- 主动健康检查任务
- Upgrade / WebSocket 隧道任务

它们都通过 `SharedState` 统一纳入生命周期管理。

退出时：

- 先等任务自然收敛
- 超时后再执行 abort

## 测试架构

当前测试大致分两层：

### 单元测试

分散在各 crate 的源文件内部，主要覆盖：

- route 匹配
- 配置校验与编译
- upstream 选择
- metrics
- rate limiting
- TLS 构造
- grpc / grpc-web 细节

### 集成测试

位于：

- `crates/rginx-app/tests/`

最近这层已经统一出共享 harness：

- [`crates/rginx-app/tests/support/mod.rs`](../crates/rginx-app/tests/support/mod.rs)

它负责：

- 启动 `rginx` 子进程
- 收集 stdout / stderr
- 基于 `/-/ready` 做 HTTP / HTTPS ready probe
- 统一超时、退出和失败时日志收集

这层统一之后，集成测试的稳定性和可维护性都比之前更好。

## 当前最值得关注的维护点

当前架构已经比早期版本清楚很多，但仍有几个维护热点值得长期关注：

- `rginx-core/src/config.rs` 仍偏大
- `rginx-config/src/compile.rs` 仍偏大
- `rginx-config/src/validate.rs` 仍偏大
- `rginx-http/src/proxy/health.rs` 仍然承载了较多健康检查与 least_conn 逻辑

这些文件不算“乱”，但属于后续继续模块化时最自然的落点。

## 继续阅读

- [Configuration](Configuration.md)
- [Routing and Handlers](Routing-and-Handlers.md)
- [Upstreams](Upstreams.md)
- [TLS and HTTP2](TLS-and-HTTP2.md)
- [Development](Development.md)
