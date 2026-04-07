# rginx

`rginx` 的产品定义是：一个面向中小规模部署的 Rust 入口反向代理，稳定支持 TLS 终止、Host/Path 路由、上游代理、基础流量治理、健康检查、热重载和可观测性。

当前正式发布线收口为 `v0.1.1`。当前正在准备下一条预发布标签 `v0.1.2-rc.4`，用于验证最近一轮测试、文档和发布流程补强。

当前明确限制、演进方向和发布前收口项见：

- [ROADMAP.md](ROADMAP.md)

如果你想看更具体的近期执行顺序，`ROADMAP.md` 里现在也包含“8 周执行路线图”。

许可证见：

- [LICENSE](LICENSE)

当前包名和可执行文件名统一为 `rginx`。

## 当前能力

- 基于 RON 配置文件启动反向代理
- 单进程多 worker 运行时，支持可配置的 tokio worker 线程数与 accept worker 数
- `Exact("/foo")` / `Prefix("/api")` 两种路由匹配，并支持按 `grpc_service` / `grpc_method` 细分 gRPC 路由
- `Proxy` / `Return` 两种处理器
- 多上游节点轮询、加权与主备转发
- 支持 `round_robin`、`ip_hash`、`least_conn` 三种 upstream 选择策略，以及 peer `weight` / `backup`
- 幂等或可重放请求的上游重试
- 入站 HTTP/2（TLS/ALPN）
- 上游 HTTP/2（HTTPS/TLS + ALPN）
- 基础 gRPC over HTTP/2 代理，透传 `application/grpc`、`TE: trailers`、请求 trailer 和响应 trailer，并支持基础 grpc-web binary/text 转换、`grpc-timeout` deadline、按 `service/method` 路由、本地代理错误到 `grpc-status` 的转换，以及下游提前取消时的 `grpc-status = 1` 可观测性
- 基础 br/gzip 响应压缩协商
- 支持 HTTP/1.1 `Upgrade` / WebSocket 透传
- 服务端侧请求/响应超时治理：`header_read_timeout_secs`、`request_body_read_timeout_secs`、`response_write_timeout_secs`
- 上游细粒度超时、连接池和 TCP/HTTP2 keepalive 调优
- 被动健康检查，以及基于 HTTP 路径或标准 gRPC health service 的主动健康检查
- 按路由做 CIDR 访问控制
- 按路由做请求速率限制
- 支持 `trusted_proxies`，可从 `X-Forwarded-For` 解析真实客户端 IP
- 自动透传或生成 `X-Request-ID`
- 支持 `server.access_log_format` 自定义 access log 模板
- 配置支持相对 `include` 和字符串环境变量展开
- 入站 TLS 终止：`server.tls`
- 出站 TLS 模式：
  - `NativeRoots`
  - `CustomCa`
  - `Insecure`
- `Ctrl-C` / `SIGTERM` 平滑退出
- `SIGHUP` 热重载配置
- `rginx check` 配置检查
- 本地只读运维面：`rginx status` / `rginx counters` / `rginx peers`（UDS）

## 专项反向代理替代合同

`rginx` 当前要替代的，不是“所有场景下的 nginx”，而是下面这条更收口的子集：

- 中小规模部署里的 HTTP / HTTPS 入口反向代理
- API gateway 前置反代
- gRPC ingress 和 grpc-web 入口转换
- 边缘节点或 LB / CDN 后置反代
- TLS 终止、Host/Path 路由、健康检查、基础流量治理和热重载

当前对外承诺应只围绕这条子集展开。某项能力要写进稳定承诺，至少应同时满足：

- 代码已实现
- 仓库内有测试覆盖
- README / ROADMAP 已写清能力边界
- 默认配置或示例配置里存在合理落点

当前明确不做的方向：

- 本地静态文件或内容分发
- 远程 HTTP 管理面、动态配置 API、公网 admin 路由
- 通用入口代理的全量 drop-in replacement
- 完整 nginx 配置语法兼容
- `stream` / `mail` / FastCGI 这类非当前主线协议能力

如果某个需求不能稳定落在上面的目标场景里，它就不应挤占当前版本线的主路径预算。

## 项目结构

`rginx` 是一个 Cargo workspace，当前核心 crate 分工如下：

| crate | 作用 |
| --- | --- |
| `crates/rginx-app` | 二进制入口、CLI、集成测试 |
| `crates/rginx-config` | 配置加载、`include` / 环境变量展开、校验、编译 |
| `crates/rginx-core` | 共享运行时模型、错误类型、upstream 选择逻辑 |
| `crates/rginx-http` | HTTP 服务、handler、proxy、限流、指标、TLS |
| `crates/rginx-runtime` | 运行时编排、信号处理、热重载、主动健康检查 |
| `crates/rginx-observability` | tracing / logging 初始化 |

当前请求主路径大致是：

`CLI -> load_and_compile -> ConfigSnapshot -> SharedState -> server accept loop -> handler::dispatch -> route action -> access log`

## 文档导航

如果你想先判断项目边界，建议优先看：

- [ROADMAP.md](ROADMAP.md)

如果你想按周推进接下来的开发工作，优先看：

- [ROADMAP.md](ROADMAP.md) 里的“8 周执行路线图”

如果你想先跑起来，优先看本 README 里的“快速开始”和“配置结构”两节。

如果你准备改代码，建议从下面这些目录开始：

- `crates/rginx-app/src`
- `crates/rginx-config/src`
- `crates/rginx-http/src`
- `crates/rginx-runtime/src`

## 快速开始

源码目录下的默认配置文件是 `configs/rginx.ron`。安装版会优先尝试固定的活跃配置 `/etc/rginx/rginx.ron`。也支持通过 `rginx_config` 或 `--config` 显式指定配置文件。

### 一键安装

从当前源码仓库安装：

```bash
./scripts/install.sh --mode source
```

安装指定 GitHub Release 版本：

```bash
curl -fsSL https://raw.githubusercontent.com/vansour/rginx/main/scripts/install.sh | bash -s -- --mode release --version <tag>
```

其中 `latest` 只会解析最新稳定版；如果你要安装预发布版，请显式传入具体 tag，例如 `v0.1.2-rc.4`。

安装脚本默认会：

- 安装 `rginx` 到 `/usr/sbin/rginx`
- 安装卸载脚本到 `/usr/sbin/rginx-uninstall`
- 安装活跃配置到 `/etc/rginx/rginx.ron`
- 安装默认站点片段到 `/etc/rginx/conf.d/default.ron`

常用参数：

```bash
./scripts/install.sh --mode source --force
```

安装完成后，默认配置路径可以直接这样验证：

```bash
rginx check
rginx -t
rginx
rginx status
```

### 一键卸载

安装完成后可以直接运行：

```bash
rginx-uninstall
```

默认会保留活跃配置目录；如果要连配置一起删除：

```bash
rginx-uninstall --purge-config
```

### 源码运行

直接运行仓库内默认配置：

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

启动前先检查配置：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
cargo run -p rginx -- -t --config configs/rginx.ron
```

如果你先构建：

```bash
cargo build -p rginx
./target/debug/rginx --config configs/rginx.ron
./target/debug/rginx check --config configs/rginx.ron
./target/debug/rginx status --config configs/rginx.ron
```

### 仓库默认配置

仓库里的 `configs/` 现在直接镜像安装后的活跃配置目录：

- `configs/rginx.ron`
- `configs/conf.d/default.ron`

## 配置结构

配置文件格式是 RON，顶层结构如下：

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
        worker_threads: None,
        accept_workers: None,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:80",
        server_names: [],
        trusted_proxies: [],
        tls: None,
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\n"),
            ),
        ),
    ],
    servers: [],
)
```

### `include` 与环境变量

配置加载阶段支持两类轻量预处理：

- 独立一行的 `// @include "relative/path.ron"`，会把目标文件内容按当前位置做文本级拼接
- 独立一行的 `// @include "conf.d/*.ron"`，会把目录下按文件名字典序排序的 `*.ron` 片段依次展开
- 双引号字符串里的 `${VAR}` 和 `${VAR:-default}`，会在解析前按进程环境变量展开

示例：

```ron
server: ServerConfig(
    listen: "${rginx_listen:-0.0.0.0:80}",
),
locations: [
    // @include "fragments/routes.ron"
],
```

边界：

- `include` 路径相对“当前包含它的配置文件所在目录”解析
- `// @include "conf.d/*.ron"` 目前只支持 `*.ron` 这种文件通配形式
- `include` 指令必须单独占一行；被包含文件内容必须在插入位置本身就是合法 RON 片段
- 环境变量只在普通双引号字符串里展开，不会替换数字、枚举名或其他非字符串 token
- 如果你需要字符串里保留字面量 `$`，使用 `$$`
- 缺失的 `${VAR}` 会直接报配置错误；`${VAR:-default}` 会使用默认值

如果你想按 nginx 风格拆站点配置，推荐让 `/etc/rginx/rginx.ron` 保留全局 `runtime / server / upstreams / locations`，并在 `servers` 里写：

```ron
servers: [
    // @include "conf.d/*.ron"
],
```

仓库默认只自带 `/etc/rginx/conf.d/default.ron` 这一个站点片段；如果你后续要继续按 nginx 风格拆更多站点，直接往 `/etc/rginx/conf.d/*.ron` 里追加新的 `VirtualHostConfig(...)` 文件即可。

### `runtime`

- `shutdown_timeout_secs`: 平滑退出等待时间，必须大于 `0`
- `worker_threads`: 可选。tokio runtime worker 线程数；未设置时使用 tokio 默认值
- `accept_workers`: 可选。监听 socket 的 accept worker 数；未设置时默认 `1`

### `server`

- `listen`: 监听地址，例如 `"0.0.0.0:80"`
- `server_names`: 可选。默认虚拟主机匹配的域名列表；为空时可作为兜底主机使用
- `trusted_proxies`: 可选。只有当 `rginx` 部署在另一层代理、LB 或 CDN 后面时才需要配置，可写单个 IP 或 CIDR
- `keep_alive`: 可选。是否启用 HTTP/1.1 keep-alive，默认 `true`
- `max_headers`: 可选。单请求头数量上限
- `max_request_body_bytes`: 可选。请求体大小上限
- `max_connections`: 可选。总并发连接数上限
- `header_read_timeout_secs`: 可选。读取请求头的超时
- `request_body_read_timeout_secs`: 可选。读取客户端请求体的超时
- `response_write_timeout_secs`: 可选。向客户端写响应的超时
- `access_log_format`: 可选。自定义 access log 输出模板，支持 `$var` / `${var}` 占位符
- `tls`: 可选。启用后由 `rginx` 直接终止入站 TLS，并通过 ALPN 自动协商 HTTP/2

`trusted_proxies` 为空时，客户端 IP 直接取 TCP 对端地址，不会信任请求里自带的 `X-Forwarded-For`。

说明：

- `request_body_read_timeout_secs` 主要用于限制慢上传或长时间停滞的 chunked/body streaming 请求
- 如果 `Proxy` 路由未显式配置 `request_body_read_timeout_secs`，请求体流式转发会继续回退到 upstream `write_timeout_secs`，以兼容现有行为
- `response_write_timeout_secs` 主要用于尽早回收长时间不读取响应的慢客户端连接
- `access_log_format` 未设置时，仍使用默认的结构化 tracing access log
- `worker_threads` 和 `accept_workers` 都是启动期参数，热重载不会变更它们

### `servers`

额外虚拟主机列表。每个 `VirtualHostConfig` 可配置：

- `server_names`
- `locations`
- `tls`

说明：

- 请求会先按 `Host` 选择虚拟主机，再在该虚拟主机内按路径匹配路由
- 如果没有任何额外虚拟主机匹配 `Host`，会回退到顶层 `server + locations` 组成的默认虚拟主机
- `server_names` 支持精确域名和 `*.example.com` 这类通配符
- 如果某个虚拟主机已命中 `Host`，但该虚拟主机里没有匹配路径，请求会返回 `404`，不会再回退默认虚拟主机

### `upstreams`

每个上游可配置：

- `name`
- `peers`
- `peers[].weight`
- `peers[].backup`
- `tls`
- `protocol`
- `load_balance`
- `server_name_override`
- `request_timeout_secs`
- `connect_timeout_secs`
- `read_timeout_secs`
- `write_timeout_secs`
- `idle_timeout_secs`
- `pool_idle_timeout_secs`
- `pool_max_idle_per_host`
- `tcp_keepalive_secs`
- `tcp_nodelay`
- `http2_keep_alive_interval_secs`
- `http2_keep_alive_timeout_secs`
- `http2_keep_alive_while_idle`
- `max_replayable_request_body_bytes`
- `unhealthy_after_failures`
- `unhealthy_cooldown_secs`
- `health_check_path`
- `health_check_grpc_service`
- `health_check_interval_secs`
- `health_check_timeout_secs`
- `healthy_successes_required`

说明：

- `peers[].url` 目前只支持 `http://` 和 `https://`
- `peers[].weight` 默认为 `1`，越大表示该 peer 希望承担更多流量
- `peers[].backup` 默认为 `false`；设为 `true` 后，该 peer 只会在主 peer 不可用时接管流量
- `protocol` 默认为 `Auto`
- `load_balance` 默认为 `RoundRobin`
- `protocol: Auto` 时，`https://` peer 会通过 ALPN 自动协商 HTTP/2；未协商到 `h2` 时回落到 HTTP/1.1
- `protocol: Http1` 会固定使用 HTTP/1.1
- `protocol: Http2` 会要求 upstream 使用 HTTP/2；当前只支持 `https://` peer，经 TLS/ALPN 建链，明文 `h2c` upstream 仍未支持
- 基础 gRPC 代理依赖 HTTP/2 upstream；当前支持 `application/grpc` 透传，以及基础 `application/grpc-web(+proto)` / `application/grpc-web-text(+proto)` 转发；grpc-web request trailer frame 也会转换为 upstream HTTP/2 request trailers
- 对 gRPC / grpc-web 请求，若下游带了 `grpc-timeout`，代理会取它与 upstream `request_timeout` 的较小值，作为整个 upstream 交互的 deadline；它同时约束等待响应头和后续响应 body 流；非法 `grpc-timeout` 会返回 `grpc-status = 3`
- 若下游在 gRPC / grpc-web 响应流结束前提前取消读取，代理会结束对应的响应转发链路；如果此时尚未观察到最终 `grpc-status`，access log 会补记为 `grpc-status = 1`
- 当 gRPC / grpc-web 请求在代理本地失败时，例如未命中路由、ACL 拒绝、限流、upstream 不可用或超时，会尽量返回对应的 `grpc-status` / `grpc-message`，而不是只返回裸 HTTP 文本错误
- `load_balance: RoundRobin` 会按 peer `weight` 做加权轮询
- `load_balance: IpHash` 会基于解析后的客户端 IP 固定主 peer，不同 peer 的命中比例会按 `weight` 倾斜；当主 peer 不健康时会按加权顺序回退
- `load_balance: LeastConn` 会结合当前活跃请求数和 peer `weight` 选择更空闲的 peer
- `backup: true` 的 peer 不参与正常主流量分配；只有主 peer 不可用时才会启用，并可作为可重试请求的后备候选
- `request_timeout_secs` 仍可用，作为兼容字段；未单独设置 `connect/read/write/idle` 时会回退到它
- `read_timeout_secs` 是推荐的新写法，对应上游响应读取超时
- `pool_idle_timeout_secs: Some(0)` 表示禁用 idle 连接过期
- `health_check_path` 可启用基于 HTTP 路径的主动健康检查
- `health_check_grpc_service` 可启用标准 gRPC health check；未显式设置 `health_check_path` 时会自动使用 `/grpc.health.v1.Health/Check`
- 启用 `health_check_grpc_service` 时，`protocol` 必须是 `Auto` 或 `Http2`，并且所有 peer 都必须是 `https://`；当前不支持明文 `h2c` gRPC health probe
- 证书、私钥、自定义 CA 等相对路径，都是相对配置文件所在目录解析

### `locations`

每个路由可配置：

- `matcher`: `Exact("/foo")` 或 `Prefix("/api")`
- `handler`: `Proxy` / `Return`
- `grpc_service`: 可选。只在 gRPC / grpc-web 请求且 service 匹配时命中
- `grpc_method`: 可选。只在 gRPC / grpc-web 请求且 method 匹配时命中
- `allow_cidrs`
- `deny_cidrs`
- `requests_per_sec`
- `burst`

说明：

- `burst` 只有在设置了 `requests_per_sec` 时才有意义
- 如果配置了 `grpc_service` / `grpc_method`，路由仍然会先经过普通路径 matcher，再在命中后继续校验 gRPC `service` / `method`
- 同一路径 matcher 下，带更多 gRPC 约束的路由优先级更高；剩余冲突按配置声明顺序处理

## 示例

### 1. 基础反向代理

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:80",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "http://127.0.0.1:9000",
                ),
            ],
            read_timeout_secs: Some(30),
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("rginx is running.\n"),
            ),
        ),
        LocationConfig(
            matcher: Prefix("/api"),
            handler: Proxy(
                upstream: "backend",
            ),
        ),
    ],
)
```

### 2. 路由限流

```ron
LocationConfig(
    matcher: Prefix("/api"),
    handler: Proxy(
        upstream: "backend",
    ),
    requests_per_sec: Some(20),
    burst: Some(10),
)
```

### 4. 信任前置代理并解析真实客户端 IP

只有在 `rginx` 前面还有一层代理时才需要这样配置：

```ron
server: ServerConfig(
    listen: "0.0.0.0:80",
    trusted_proxies: [
        "10.0.0.0/8",
        "192.168.0.0/16",
        "127.0.0.1/32",
    ],
),
```

启用后，如果连接对端属于 `trusted_proxies`，`rginx` 会从 `X-Forwarded-For` 链中选取最后一个非受信代理 IP 作为客户端真实地址，并用于日志、ACL 和限流。

### 5. 入站 TLS 终止

```ron
server: ServerConfig(
    listen: "0.0.0.0:443",
    tls: Some((
        cert_path: "./certs/fullchain.pem",
        key_path: "./certs/privkey.pem",
    )),
),
```

等价的完整写法是：

```ron
server: ServerConfig(
    listen: "0.0.0.0:443",
    tls: Some(ServerTlsConfig(
        cert_path: "./certs/fullchain.pem",
        key_path: "./certs/privkey.pem",
    )),
),
```

证书和私钥需要是 PEM 文件。

### 6. 多虚拟主机

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:80",
        server_names: ["default.example.com"],
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("default site\n"),
            ),
        ),
    ],
    servers: [
        VirtualHostConfig(
            server_names: ["api.example.com"],
            locations: [
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("api root\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Prefix("/v1"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("api v1\n"),
                    ),
                ),
            ],
        ),
        VirtualHostConfig(
            server_names: ["*.internal.example.com"],
            locations: [
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("internal site\n"),
                    ),
                ),
            ],
        ),
    ],
)
```

行为规则：

- `Host: api.example.com` 会进入 `api.example.com` 对应的虚拟主机
- `Host: app.internal.example.com` 会命中通配符虚拟主机
- 未命中任何 `server_names` 时，回退到顶层默认虚拟主机
- `Host` 里的端口会被忽略，例如 `api.example.com:8080`

### 7. 上游 HTTPS

使用系统根证书：

```ron
UpstreamConfig(
    name: "example-secure",
    peers: [
        UpstreamPeerConfig(
            url: "https://example.com",
        ),
    ],
    tls: Some(NativeRoots),
    protocol: Auto,
    server_name_override: Some("example.com"),
)
```

使用自定义 CA：

```ron
UpstreamConfig(
    name: "dev-secure",
    peers: [
        UpstreamPeerConfig(
            url: "https://localhost:9443",
        ),
    ],
    tls: Some(CustomCa(
        ca_cert_path: "./certs/dev-ca.pem",
    )),
    protocol: Auto,
)
```

跳过证书校验，仅建议本地调试：

```ron
UpstreamConfig(
    name: "dev-insecure",
    peers: [
        UpstreamPeerConfig(
            url: "https://localhost:9443",
        ),
    ],
    tls: Some(Insecure),
    protocol: Http2,
    server_name_override: Some("localhost"),
)
```

### 8. 上游超时与连接池

```ron
UpstreamConfig(
    name: "backend",
    peers: [
        UpstreamPeerConfig(
            url: "https://api.internal.example",
        ),
    ],
    protocol: Auto,
    connect_timeout_secs: Some(3),
    read_timeout_secs: Some(30),
    write_timeout_secs: Some(30),
    idle_timeout_secs: Some(60),
    pool_idle_timeout_secs: Some(90),
    pool_max_idle_per_host: Some(64),
    tcp_keepalive_secs: Some(30),
    tcp_nodelay: Some(true),
    http2_keep_alive_interval_secs: Some(15),
    http2_keep_alive_timeout_secs: Some(20),
    http2_keep_alive_while_idle: Some(true),
)
```

常见建议：

- `connect_timeout_secs` 用较小值，尽快切换到其他 peer
- `read_timeout_secs` / `write_timeout_secs` 用于限制慢 upstream
- `idle_timeout_secs` 用于限制长时间无进展的响应流
- `pool_idle_timeout_secs` 控制连接池内空闲连接保留时长
- `pool_max_idle_per_host` 控制每个 upstream host 的最大空闲连接数

### 9. 基于客户端 IP 的粘性转发

```ron
server: ServerConfig(
    listen: "0.0.0.0:80",
    trusted_proxies: ["127.0.0.1/32"],
),

upstreams: [
    UpstreamConfig(
        name: "backend",
        peers: [
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9000",
            ),
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9001",
            ),
        ],
        load_balance: IpHash,
    ),
],
```

适用场景：

- 需要把同一客户端尽量打到同一台应用节点
- 上游本身没有共享 session，或者共享成本较高
- `rginx` 前面还有 LB / CDN 时，要正确配置 `trusted_proxies`，否则 hash 的会是前置代理 IP

### 10. 最少连接数转发

```ron
upstreams: [
    UpstreamConfig(
        name: "backend",
        peers: [
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9000",
            ),
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9001",
            ),
        ],
        load_balance: LeastConn,
    ),
],
```

适用场景：

- 上游请求耗时差异较大，希望把新请求优先打到当前更空闲的 peer
- 有长轮询、流式响应或升级连接，不适合简单轮询
- 需要比 `RoundRobin` 更贴近真实负载的分配方式

### 11. 加权 upstream

```ron
upstreams: [
    UpstreamConfig(
        name: "backend",
        peers: [
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9000",
                weight: 3,
            ),
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9001",
                weight: 1,
            ),
        ],
    ),
],
```

适用场景：

- 新老节点并存时，希望大部分流量先打到容量更高的新节点
- 需要按机器规格差异分流，例如 8C 机器比 2C 机器承担更多请求
- 希望在不改业务服务发现的情况下，直接在入口层做简单容量倾斜

### 12. Backup upstream

```ron
upstreams: [
    UpstreamConfig(
        name: "backend",
        peers: [
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9000",
            ),
            UpstreamPeerConfig(
                url: "http://127.0.0.1:9001",
                backup: true,
            ),
        ],
        request_timeout_secs: Some(1),
        unhealthy_after_failures: Some(1),
        unhealthy_cooldown_secs: Some(30),
    ),
],
```

适用场景：

- 主节点集群正常时不希望流量打到备用节点
- 主节点超时或进入冷却后，希望自动切到灾备节点
- 需要在入口层做简单主备切换，而不是把备机长期纳入常规负载均衡

### 13. 请求 ID

`rginx` 会优先复用入站请求自带的 `X-Request-ID`；如果客户端没有带，它会自动生成类似 `rginx-0000000000000001` 的 ID，并同时：

- 透传给 upstream
- 回写到下游响应头
- 记录到 access log

这样可以把边缘日志、应用日志和调用链串起来。

### 14. 自定义 Access Log

可以在 `server` 下配置：

```ron
ServerConfig(
    listen: "0.0.0.0:80",
    access_log_format: Some("reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method status=$status grpc_status=$grpc_status request=\"$request\" bytes=$body_bytes_sent elapsed=$request_time_ms"),
)
```

当前支持的占位符包括：

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

说明：

- `grpc_status` / `grpc_message` 会优先取最终 gRPC trailers；如果只有响应头里可见，也会回退读取响应头
- 对 `grpc-web` / `grpc-web-text`，`rginx` 会从最终 trailer frame 提取 `grpc_status` / `grpc_message`，用于 access log
- 若下游在响应流结束前提前取消，且代理尚未观察到最终 `grpc-status`，则会将 access log 里的 `grpc_status` 补记为 `1`

兼容别名：

- `$client_ip` = `$remote_addr`
- `$request_method` = `$method`
- `$request_uri` = `$path`
- `$bytes_sent` = `$body_bytes_sent`
- `$elapsed_ms` = `$request_time_ms`
- `$server_name` = `$vhost`
- `$server_protocol` = `$http_version`
- `$user_agent` = `$http_user_agent`
- `$referer` = `$http_referer`

## 维护与发版

### 日常 CI

仓库内置了独立的 CI workflow：

- `.github/workflows/ci.yml`

触发条件：

- `pull_request`
- push 到 `main`
- 手动 `workflow_dispatch`

CI 会在 `ubuntu-24.04` 上执行：

- `cargo fmt --all --check`
- `cargo test --workspace --locked --quiet`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`

### 如何发版

仓库内置了 tag 驱动的发布 workflow：

- `.github/workflows/release.yml`

触发方式：

```bash
git tag v1.2.3
git push origin v1.2.3
```

tag 被 push 之后，GitHub Actions 会自动：

- 校验 tag 格式是否符合 `v*`
- 重新执行格式检查和全量测试
- 构建多架构发布产物
- 自动创建或更新同名 GitHub Release
- 在 Release 页面写入当前 tag 对应的 commit、上一个 tag、compare 链接和本次发布的具体 changelog
- 上传二进制压缩包和 `SHA256SUMS.txt`

当前发布矩阵：

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`

每个 release archive 现在会同时包含：

- `rginx`
- `configs/`
- `scripts/install.sh`
- `scripts/uninstall.sh`
- `scripts/prepare-release.sh`
- `README.md`
- `LICENSE*`

Release Notes 分类规则来自：

- `.github/release.yml`

建议的本地发版前检查：

```bash
./scripts/prepare-release.sh --tag v0.1.2-rc.4
```

建议流程：

1. 确认 `main` 上的 CI 通过。
2. 确认工作区没有误提交的临时改动。
3. 在目标提交上执行 `./scripts/prepare-release.sh --tag <tag>`。
4. 创建语义化版本 tag，例如 `v1.2.3` 或 `v1.2.3-rc.1`。
5. push tag，等待 `release.yml` 完成。
6. 到 GitHub Release 页面检查生成的 changelog、commit 链接和各平台产物是否齐全。

说明：

- 即使这是仓库的第一个 tag，没有“上一个版本”可比较，release workflow 也会基于当前 tag 所包含的提交历史自动写出 `## Changelog`
- 如果存在上一个 tag，`## Changelog` 会列出从上一个 tag 到当前 tag 的具体提交
- 预发布 tag 允许指向已被 `origin/main` 包含的历史提交；正式版 tag 仍应直接切在 `origin/main` 当前 HEAD 上

产物命名规则示例：

- `rginx-v1.2.3-linux-amd64.tar.gz`
- `rginx-v1.2.3-linux-arm64.tar.gz`

## 运维操作

如果你需要更具体的“安装布局、管理接口隔离、外部 supervisor 托管”建议，可以直接按下面的约定部署：

- 默认前缀 `/usr` 时使用 `/usr/sbin/rginx`
- 活跃配置使用 `/etc/rginx/rginx.ron`
- 站点拆分配置放在 `/etc/rginx/conf.d/*.ron`

### 热重载

向进程发送 `SIGHUP` 会重新加载配置：

```bash
kill -HUP <pid>
```

也支持 nginx 风格命令：

```bash
rginx -s reload
```

当前热重载有几个启动期限制：`listen`、`runtime.worker_threads` 和 `runtime.accept_workers` 不能在重载时修改，变更这些参数需要重启进程。

### 平滑退出

以下信号会触发平滑退出：

- `Ctrl-C`
- `SIGTERM`
- `SIGQUIT`

`rginx` 会停止接受新连接，并在 `runtime.shutdown_timeout_secs` 内等待已有请求排空。

也支持 nginx 风格命令：

```bash
rginx -s quit
rginx -s stop
```

### 本地只读状态

`rginx` 不再暴露公网 HTTP 管理路由；当前版本线的本地只读运维入口改为 UDS + CLI。

常用命令：

```bash
rginx status
rginx counters
rginx peers
```

默认安装路径下，运行时会使用：

```text
/run/rginx/admin.sock
```

如果你使用自定义配置路径，例如 `/tmp/site.ron`，对应的本地 admin socket 会落在：

```text
/tmp/site.admin.sock
```

这条本地只读面当前提供四类查询能力：

- `GetStatus`
- `GetCounters`
- `GetPeerHealth`
- `GetRevision`

它的目标是服务本机 CLI 和后续 `edge-agent` 集成，而不是重新引入远程管理面。

## 当前限制

- 明文 HTTP/2（h2c）入站仍未支持
- 明文 HTTP/2（h2c）upstream 仍未支持
- 当前支持基础 `grpc-web` 二进制和 text 模式，也支持将下游提前取消补记为 `grpc-status = 1`；明文 `h2c` gRPC upstream 与更完整的高级 gRPC 语义（例如更主动的 cancellation 协同或更完整的协议级兼容）仍未支持
- 主动 gRPC health check 当前只支持 `https://` upstream，不支持明文 `h2c`
- 更完整的高级压缩策略当前仍未支持（目前只支持基础 br/gzip 协商）
- 配置变更当前只支持“修改本地配置文件 + `rginx check` + reload”，不提供远程动态配置 API 或 partial patch
- `SO_REUSEPORT` 多进程 worker 形态当前仍未支持
- 热重载不能切换监听地址、`runtime.worker_threads` 或 `runtime.accept_workers`
- 本地只读运维面当前只提供 UDS + CLI，不提供远程查询协议、分页或 watch 流

更细致的能力矩阵、工程演进观察和建议阶段规划，见 [ROADMAP.md](ROADMAP.md)。

## 目标定位

如果你的目标是“在中小规模部署里使用一个聚焦入口层的 Rust 反向代理”，当前这版已经覆盖了最核心的一批能力：

- 反向代理
- TLS 终止
- 上游 HTTPS
- Host/Path 路由
- ACL 与限流
- 健康检查
- 平滑退出与热重载

更准确地说，它当前的定位是：

- 先把 nginx 最常见的反向代理子集做扎实，而不是追求全量兼容
- 先把本地可运维性、代理能力和上线可信度补齐，而不是继续扩展非主线功能
- 先服务 API / gRPC / 边缘入口这些高频专项场景，而不是泛化成“什么都做一点”的入口层
