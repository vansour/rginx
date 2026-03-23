# Configuration

本页说明 `Rginx` 的配置模型、字段分层和几个容易踩坑的点。

## 配置加载流程

配置文件通过下面的链路进入运行时：

1. `crates/rginx-config/src/load.rs`
   - 从磁盘读取 RON 文本
2. `crates/rginx-config/src/validate.rs`
   - 校验字段值和跨字段约束
3. `crates/rginx-config/src/compile.rs`
   - 补默认值
   - 解析路径
   - 编译为 `ConfigSnapshot`

配置错误会尽量在启动或 `rginx check` 阶段暴露，不会拖到第一条请求才发现。

## 顶层结构

```ron
Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 10,
    ),
    server: ServerConfig(
        listen: "0.0.0.0:8080",
    ),
    upstreams: [],
    locations: [],
    servers: [],
)
```

| 字段 | 作用 |
| --- | --- |
| `runtime` | 运行时行为，例如平滑退出超时 |
| `server` | 默认监听器和默认虚拟主机的服务级配置 |
| `upstreams` | 所有可代理的上游集群 |
| `locations` | 默认虚拟主机的路由列表 |
| `servers` | 额外虚拟主机列表 |

## `RuntimeConfig`

| 字段 | 含义 |
| --- | --- |
| `shutdown_timeout_secs` | 平滑退出等待时间，必须大于 `0` |

## `ServerConfig`

| 字段 | 含义 |
| --- | --- |
| `listen` | 监听地址，例如 `127.0.0.1:8080` |
| `server_names` | 默认虚拟主机可匹配的域名列表 |
| `trusted_proxies` | 受信代理 CIDR，用于解析真实客户端 IP |
| `keep_alive` | 是否启用 HTTP/1.1 keep-alive |
| `max_headers` | 单请求头数量上限 |
| `max_request_body_bytes` | 请求体大小上限 |
| `max_connections` | 服务端总连接数上限 |
| `header_read_timeout_secs` | 读取请求头的超时 |
| `tls` | 入站 TLS 证书与私钥 |

注意：

- `trusted_proxies` 为空时，客户端 IP 永远取 TCP 对端地址。
- `listen` 变更不能通过热重载生效，必须重启。

## `VirtualHostConfig`

每个虚拟主机包含：

- `server_names`
- `locations`
- `tls`

规则：

- 请求先按 `Host` 选择虚拟主机，再按路径选择路由。
- 如果命中某个虚拟主机但该虚拟主机里没有匹配路由，请求会返回 `404`，不会回退到默认虚拟主机。
- `server_names` 支持精确域名和 `*.example.com` 通配符。

## `UpstreamConfig`

`upstream` 的详细行为见 [Upstreams](Upstreams.md)，这里只列出结构：

| 字段 | 含义 |
| --- | --- |
| `name` | upstream 名称 |
| `peers` | peer 列表 |
| `tls` | 上游 TLS 模式 |
| `protocol` | `Auto` / `Http1` / `Http2` |
| `load_balance` | `RoundRobin` / `IpHash` / `LeastConn` |
| `server_name_override` | 上游 TLS SNI 覆盖 |
| `request_timeout_secs` | 兼容字段，作为多种超时回退值 |
| `connect_timeout_secs` | 建连超时 |
| `read_timeout_secs` | 读取响应头/响应体的请求级超时入口 |
| `write_timeout_secs` | 写入请求体超时 |
| `idle_timeout_secs` | 响应体空闲超时 |
| `pool_idle_timeout_secs` | 连接池空闲连接保留时长 |
| `pool_max_idle_per_host` | 每个 host 最大空闲连接数 |
| `tcp_keepalive_secs` | 上游 TCP keepalive |
| `tcp_nodelay` | 是否启用 TCP_NODELAY |
| `http2_keep_alive_interval_secs` | 上游 HTTP/2 keepalive 探测间隔 |
| `http2_keep_alive_timeout_secs` | HTTP/2 keepalive 超时 |
| `http2_keep_alive_while_idle` | 连接空闲时是否继续发 keepalive |
| `max_replayable_request_body_bytes` | 可重放请求体大小上限 |
| `unhealthy_after_failures` | 被动健康检查的失败阈值 |
| `unhealthy_cooldown_secs` | 进入冷却期的时长 |
| `health_check_path` | 主动健康检查路径 |
| `health_check_interval_secs` | 主动检查间隔 |
| `health_check_timeout_secs` | 主动检查超时 |
| `healthy_successes_required` | 从不健康恢复到健康所需成功次数 |

### `UpstreamPeerConfig`

| 字段 | 含义 |
| --- | --- |
| `url` | 只支持 `http://` 和 `https://` |
| `weight` | 默认为 `1` |
| `backup` | 默认为 `false`，设为 `true` 后只在主 peer 不可用时启用 |

## `LocationConfig`

| 字段 | 含义 |
| --- | --- |
| `matcher` | `Exact("/foo")` 或 `Prefix("/api")` |
| `handler` | 路由处理器 |
| `allow_cidrs` | 允许名单 |
| `deny_cidrs` | 拒绝名单 |
| `requests_per_sec` | 每秒请求数 |
| `burst` | 突发桶大小 |

`handler` 支持：

- `Static`
- `Proxy`
- `File`
- `Return`
- `Status`
- `Metrics`

详细行为见 [Routing and Handlers](Routing-and-Handlers.md)。

## 默认值与常见约束

几个最常见的默认值：

- `load_balance` 默认 `RoundRobin`
- `protocol` 默认 `Auto`
- `peers[].weight` 默认 `1`
- `peers[].backup` 默认 `false`
- `pool_idle_timeout_secs` 默认 90 秒
- `unhealthy_after_failures` 默认 2
- `unhealthy_cooldown_secs` 默认 10 秒
- 主动健康检查 interval 默认 5 秒
- 主动健康检查 timeout 默认 2 秒
- 恢复为健康所需成功次数默认 2

常见约束：

- 绝大多数 timeout / limit 不能为 `0`
- `health_check_interval_secs` / `health_check_timeout_secs` / `healthy_successes_required` 只有在设置了 `health_check_path` 时才有意义
- `protocol = Http2` 时，当前要求所有 peer 都是 `https://`
- `pool_idle_timeout_secs = Some(0)` 表示关闭 idle 过期，而不是非法值

## 路径解析规则

相对路径都相对“配置文件所在目录”解析，而不是当前 shell 工作目录。典型字段包括：

- `server.tls.cert_path`
- `server.tls.key_path`
- `upstream.tls.CustomCa.ca_cert_path`
- `File.root`

## 路由顺序是否重要

配置文件里写的顺序不会直接决定匹配优先级。编译阶段会按 matcher 优先级重新排序：

- `Exact` 优先于 `Prefix`
- 更长的路径优先于更短的路径

这意味着你可以按“可读性”组织配置，而不是靠手工调顺序避冲突。

## 推荐阅读

- [Routing and Handlers](Routing-and-Handlers.md)
- [Upstreams](Upstreams.md)
- [TLS and HTTP2](TLS-and-HTTP2.md)
