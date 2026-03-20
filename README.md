# Rginx

`Rginx` 是一个用 Rust 编写的边缘反向代理，目标是直接替代 Nginx 处理中小规模部署里最常见的入口层能力：

- 路由转发与静态响应
- TLS 终止
- 上游 HTTPS 访问
- 基础访问控制与限流
- 健康检查、状态页与 Prometheus 指标
- 平滑退出与热重载

当前包名和可执行文件名统一为 `rginx`。

## 当前能力

- 基于 RON 配置文件启动反向代理
- `Exact("/foo")` / `Prefix("/api")` 两种路由匹配
- `Static` / `Proxy` / `Status` / `Metrics` 四种处理器
- 多上游节点轮询转发
- 幂等或可重放请求的上游重试
- 被动健康检查与主动健康检查
- 按路由做 CIDR 访问控制
- 按路由做请求速率限制
- 支持 `trusted_proxies`，可从 `X-Forwarded-For` 解析真实客户端 IP
- 入站 TLS 终止：`server.tls`
- 出站 TLS 模式：
  - `NativeRoots`
  - `CustomCa`
  - `Insecure`
- `/status` JSON 状态接口
- `/metrics` Prometheus 指标接口
- `Ctrl-C` / `SIGTERM` 平滑退出
- `SIGHUP` 热重载配置
- `rginx check` 配置检查

## 快速开始

默认配置文件是 `configs/rginx.ron`。

```bash
cargo run -p rginx -- --config configs/rginx.ron
```

启动前先检查配置：

```bash
cargo run -p rginx -- check --config configs/rginx.ron
```

如果你先构建：

```bash
cargo build -p rginx
./target/debug/rginx --config configs/rginx.ron
./target/debug/rginx check --config configs/rginx.ron
```

仓库已自带几个示例配置：

- `configs/rginx.ron`
- `configs/rginx-https-example.ron`
- `configs/rginx-https-custom-ca-example.ron`
- `configs/rginx-https-insecure-example.ron`

## 配置结构

配置文件格式是 RON，顶层结构如下：

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
        trusted_proxies: [],
        tls: None,
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Static(
                body: "ok\n",
            ),
        ),
    ],
)
```

### `runtime`

- `shutdown_timeout_secs`: 平滑退出等待时间，必须大于 `0`

### `server`

- `listen`: 监听地址，例如 `"0.0.0.0:8080"`
- `trusted_proxies`: 可选。只有当 `Rginx` 部署在另一层代理、LB 或 CDN 后面时才需要配置，可写单个 IP 或 CIDR
- `tls`: 可选。启用后由 `Rginx` 直接终止入站 TLS

`trusted_proxies` 为空时，客户端 IP 直接取 TCP 对端地址，不会信任请求里自带的 `X-Forwarded-For`。

### `upstreams`

每个上游可配置：

- `name`
- `peers`
- `tls`
- `server_name_override`
- `request_timeout_secs`
- `max_replayable_request_body_bytes`
- `unhealthy_after_failures`
- `unhealthy_cooldown_secs`
- `health_check_path`
- `health_check_interval_secs`
- `health_check_timeout_secs`
- `healthy_successes_required`

说明：

- `peers[].url` 目前只支持 `http://` 和 `https://`
- `health_check_path` 启用主动健康检查
- 证书、私钥、自定义 CA 等相对路径，都是相对配置文件所在目录解析

### `locations`

每个路由可配置：

- `matcher`: `Exact("/foo")` 或 `Prefix("/api")`
- `handler`: `Static` / `Proxy` / `Status` / `Metrics`
- `allow_cidrs`
- `deny_cidrs`
- `requests_per_sec`
- `burst`

说明：

- `burst` 只有在设置了 `requests_per_sec` 时才有意义

## 示例

### 1. 基础反向代理

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "http://127.0.0.1:9000",
                ),
            ],
            request_timeout_secs: Some(30),
        ),
    ],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Static(
                status: Some(200),
                content_type: Some("text/plain; charset=utf-8"),
                body: "Rginx is running.\n",
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

### 2. 状态页、指标与 ACL

```ron
locations: [
    LocationConfig(
        matcher: Exact("/status"),
        handler: Status,
        allow_cidrs: ["127.0.0.1/32", "::1/128"],
    ),
    LocationConfig(
        matcher: Exact("/metrics"),
        handler: Metrics,
        allow_cidrs: ["127.0.0.1/32", "::1/128"],
    ),
]
```

`/status` 会返回 JSON，包含：

- 当前配置修订号 `revision`
- 监听地址 `listen`
- 路由数与上游数
- 每个上游的请求超时、主动健康检查参数、每个 peer 的健康状态

`/metrics` 会暴露 Prometheus 文本格式指标，包含：

- `rginx_active_connections`
- `rginx_http_requests_total`
- `rginx_http_rate_limited_total`
- `rginx_http_request_duration_ms`
- `rginx_upstream_requests_total`
- `rginx_active_health_checks_total`
- `rginx_config_reloads_total`

### 3. 路由限流

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

只有在 `Rginx` 前面还有一层代理时才需要这样配置：

```ron
server: ServerConfig(
    listen: "0.0.0.0:8080",
    trusted_proxies: [
        "10.0.0.0/8",
        "192.168.0.0/16",
        "127.0.0.1/32",
    ],
),
```

启用后，如果连接对端属于 `trusted_proxies`，`Rginx` 会从 `X-Forwarded-For` 链中选取最后一个非受信代理 IP 作为客户端真实地址，并用于日志、ACL 和限流。

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

### 6. 上游 HTTPS

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
)
```

## 运维操作

### 热重载

向进程发送 `SIGHUP` 会重新加载配置：

```bash
kill -HUP <pid>
```

当前热重载有一个限制：`listen` 地址不能在重载时修改，变更监听端口需要重启进程。

### 平滑退出

以下信号会触发平滑退出：

- `Ctrl-C`
- `SIGTERM`

`Rginx` 会停止接受新连接，并在 `runtime.shutdown_timeout_secs` 内等待已有请求排空。

## 当前限制

- 入站连接目前只支持 HTTP/1.x
- 还没有 HTTP/2 支持
- 还没有 WebSocket / `Upgrade` 代理支持
- 热重载不能切换监听地址

## 目标定位

如果你的目标是“直接拿它放到入口层，替掉一部分 Nginx 反代场景”，当前这版已经覆盖了最核心的一批能力：

- 反向代理
- TLS 终止
- 上游 HTTPS
- 状态与指标
- ACL 与限流
- 健康检查
- 平滑退出与热重载

但它还不是完整的 Nginx 等价物。HTTP/2、WebSocket、更丰富的负载均衡策略和更完整的运维能力，仍然需要继续补齐。
