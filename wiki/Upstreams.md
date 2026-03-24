# Upstreams

本页说明 `rginx` 的 upstream 模型、负载均衡策略、故障转移与健康检查。

## 基本模型

一个 upstream 由以下几部分组成：

- upstream 名称
- peer 列表
- 负载均衡策略
- TLS / HTTP 协议配置
- 超时与连接池参数
- 被动健康检查参数
- 主动健康检查参数

运行时中，proxy handler 只通过 upstream 名称引用 upstream。

## Peer 字段

每个 peer 支持：

- `url`
- `weight`
- `backup`

语义：

- `weight`
  - 默认 `1`
  - 值越大，越倾向承担更多流量
- `backup`
  - 默认 `false`
  - `true` 表示这是备节点，不参与正常主流量分配
  - 只有主 peer 池不可用时，才会接管流量

## 负载均衡策略

### `RoundRobin`

默认策略。

行为：

- 在主 peer 池内按 `weight` 加权轮询
- 如果主 peer 池不可用，再切到 backup peer 池

适合：

- 绝大多数无状态服务
- peer 性能相近的集群

### `IpHash`

基于客户端 IP 做稳定选择。

行为：

- 先解析真实客户端 IP
- 在主 peer 池内做稳定 hash
- `weight` 会影响命中比例
- 主 peer 不健康时，按同池顺序回退
- 主池整体不可用时，切到 backup 池

适合：

- session 粘性
- 本地缓存命中优化

### `LeastConn`

按当前活跃请求数选择更空闲的节点。

行为：

- 只在当前可用的主 peer 中选
- 同时考虑 `active_requests` 与 `weight`
- 当投影负载接近时，更高权重的 peer 会优先承担更多请求
- 主池不可用时，切到 backup 池

适合：

- 请求时长差异较大
- SSE、长轮询、Upgrade 连接较多

## `weight` 与 `backup` 的组合语义

几个最重要的规则：

1. `backup = true` 的 peer 不会参与主池正常分流。
2. `weight` 只影响所在池内的流量分配。
3. backup peer 也会保留自己的 `weight`，用于“多个备节点之间”的分配。

例如：

```ron
peers: [
    UpstreamPeerConfig(
        url: "http://10.0.0.10:8080",
        weight: 4,
    ),
    UpstreamPeerConfig(
        url: "http://10.0.0.11:8080",
        weight: 2,
    ),
    UpstreamPeerConfig(
        url: "http://10.0.1.10:8080",
        backup: true,
        weight: 3,
    ),
    UpstreamPeerConfig(
        url: "http://10.0.1.11:8080",
        backup: true,
        weight: 1,
    ),
]
```

上面的语义是：

- 正常情况下只在前两个 peer 之间分流
- 主池整体不可用时，才在两个 backup peer 之间按 `3:1` 分流

## 重试与 failover

`rginx` 只会对“幂等或可重放”的请求做 peer 级 failover。

当前规则：

- 幂等方法才有资格重试：
  - `GET`
  - `HEAD`
  - `PUT`
  - `DELETE`
  - `OPTIONS`
  - `TRACE`
- 请求体必须可重放：
  - 空 body
  - 或者已缓冲在内存里且未超过 `max_replayable_request_body_bytes`

这意味着：

- 普通 `POST` 默认不会自动切 peer 重试
- 大流式请求也不会被透明重放

## 超时模型

常用字段：

- `request_timeout_secs`
- `connect_timeout_secs`
- `read_timeout_secs`
- `write_timeout_secs`
- `idle_timeout_secs`

建议理解为三层：

1. 建连超时
2. 请求体写入超时
3. 响应体空闲超时

`request_timeout_secs` 仍然保留，主要用作兼容字段和回退值。

## 连接池与 TCP/HTTP2 参数

可调项：

- `pool_idle_timeout_secs`
- `pool_max_idle_per_host`
- `tcp_keepalive_secs`
- `tcp_nodelay`
- `http2_keep_alive_interval_secs`
- `http2_keep_alive_timeout_secs`
- `http2_keep_alive_while_idle`

适合调整的场景：

- 上游很多短连接，想提升复用率
- 上游是云 LB，需要更积极的 keepalive
- 上游是 HTTP/2 服务，希望更稳定地保活

## 被动健康检查

被动健康检查直接依赖真实流量结果。

关键字段：

- `unhealthy_after_failures`
- `unhealthy_cooldown_secs`

逻辑：

1. 某个 peer 发生连续失败时，累计失败次数。
2. 达到阈值后，把该 peer 标记为不健康并进入冷却。
3. 冷却期内，该 peer 不会被选择。
4. 冷却期结束后，peer 会重新进入可选状态。

## 主动健康检查

主动健康检查由后台任务定期探测。

关键字段：

- `health_check_path`
- `health_check_grpc_service`
- `health_check_interval_secs`
- `health_check_timeout_secs`
- `healthy_successes_required`

逻辑：

1. 对每个配置了 `health_check_path` 或 `health_check_grpc_service` 的 peer 周期性发起探测请求。
2. 失败会把 peer 标记为主动不健康。
3. 连续成功达到阈值后，再恢复为健康。

主动检查适用于：

- 上游有明确的 `/healthz` / `/ready` 路径
- 希望在真实用户流量到来前提前摘除故障节点

### gRPC 主动健康检查

当配置 `health_check_grpc_service` 时，`rginx` 会发送标准 gRPC health check：

- method path 固定为 `/grpc.health.v1.Health/Check`
- 如果未显式写 `health_check_path`，编译阶段会自动补这个默认 path
- request body 使用 `grpc.health.v1.HealthCheckRequest`
- 成功判定收口为：
  - HTTP 状态码成功
  - `grpc-status = 0`
  - response body 中的 serving status = `SERVING`

当前约束：

- 只支持 `https://` peer
- 只支持 `protocol = Auto` 或 `Http2`
- 不支持明文 `h2c` gRPC health probe
- 不支持自定义 gRPC health method path

## backup peer 与健康检查

backup peer 也会参与健康状态维护：

- 被动健康检查一样会累计失败
- 配置了主动探测时，一样会被周期性检查

区别只在于“是否参与正常流量分配”，不是“是否参与健康管理”。

## `/status` 可看到什么

`/status` 中每个 peer 都会包含：

- `url`
- `weight`
- `backup`
- `healthy`
- `active_requests`
- 被动失败计数
- 被动冷却剩余时间
- 主动健康状态

这对排查下面的问题很有用：

- 为什么流量没有打到某个 peer
- 某个 peer 是否处在冷却期
- `least_conn` 下哪台机器当前更忙
- backup 是否已经开始接流量

## 推荐阅读

- [TLS and HTTP2](TLS-and-HTTP2.md)
- [Operations](Operations.md)
- [Examples](Examples.md)
